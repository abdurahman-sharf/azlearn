-- Platform core (phase 0): profiles + roles, institutions tree, subjects, audit log.
-- Security model: roles/status are NEVER writable by the client; RLS enforces it.

-- ───────── profiles ─────────
create table public.profiles (
  id               uuid primary key references auth.users (id) on delete cascade,
  full_name        text not null default '' check (char_length(full_name) <= 120),
  role             text not null default 'student'
                   check (role in ('admin', 'institution_admin', 'moderator', 'teacher', 'student')),
  institution_type text check (institution_type in ('school', 'institute', 'university')),
  status           text not null default 'active'
                   check (status in ('active', 'pending', 'rejected', 'suspended')),
  status_reason    text check (char_length(status_reason) <= 500),
  bio              text check (char_length(bio) <= 1000),
  avatar_url       text,
  created_at       timestamptz not null default now(),
  updated_at       timestamptz not null default now()
);

alter table public.profiles enable row level security;

-- Helper (must come after profiles: SQL function bodies are validated at creation).
create or replace function public.is_admin()
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (
    select 1 from public.profiles
    where id = auth.uid() and role = 'admin' and status = 'active'
  );
$$;

-- Create the profile when an auth user is created. The client may only ask for
-- 'student' or 'teacher'; teachers start as 'pending' until an admin approves.
-- Admin accounts are created manually (see supabase/README.md).
create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
declare
  req_role text := new.raw_user_meta_data ->> 'role';
  req_type text := new.raw_user_meta_data ->> 'institution_type';
  final_role text := case when req_role = 'teacher' then 'teacher' else 'student' end;
begin
  insert into public.profiles (id, full_name, role, institution_type, status)
  values (
    new.id,
    left(coalesce(new.raw_user_meta_data ->> 'full_name', ''), 120),
    final_role,
    case when req_type in ('school', 'institute', 'university') then req_type end,
    case when final_role = 'teacher' then 'pending' else 'active' end
  );
  return new;
end;
$$;

create trigger on_auth_user_created
  after insert on auth.users
  for each row execute function public.handle_new_user();

-- Block privilege escalation: only admins (or the service role / SQL editor,
-- where auth.uid() is null) may change role, status or status_reason.
create or replace function public.guard_profile_update()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  new.updated_at := now();
  if auth.uid() is not null and not public.is_admin() then
    if new.role is distinct from old.role
       or new.status is distinct from old.status
       or new.status_reason is distinct from old.status_reason
       or new.id is distinct from old.id then
      raise exception 'not allowed to change role or status' using errcode = '42501';
    end if;
  end if;
  return new;
end;
$$;

create trigger guard_profile_update
  before update on public.profiles
  for each row execute function public.guard_profile_update();

create policy "own profile or admin can read"
  on public.profiles for select to authenticated
  using (id = auth.uid() or public.is_admin());

-- Approved teachers are browsable by any signed-in user (students browse teachers).
create policy "approved teachers are visible"
  on public.profiles for select to authenticated
  using (role = 'teacher' and status = 'active');

create policy "users update own profile"
  on public.profiles for update to authenticated
  using (id = auth.uid())
  with check (id = auth.uid());

create policy "admin updates any profile"
  on public.profiles for update to authenticated
  using (public.is_admin())
  with check (public.is_admin());

-- No insert policy (rows come from the trigger); no delete policy (cascade from auth.users).

-- ───────── institutions ─────────
create table public.institutions (
  id         uuid primary key default gen_random_uuid(),
  type       text not null check (type in ('school', 'institute', 'university')),
  name_ar    text not null check (char_length(name_ar) between 1 and 200),
  name_en    text check (char_length(name_en) <= 200),
  city       text check (char_length(city) <= 100),
  is_active  boolean not null default true,
  created_at timestamptz not null default now()
);
create index institutions_type_idx on public.institutions (type) where is_active;

-- Flexible tree: school: level(grade) > year > term; institute/university: department > level > year > term.
create table public.org_units (
  id             uuid primary key default gen_random_uuid(),
  institution_id uuid not null references public.institutions (id) on delete cascade,
  parent_id      uuid references public.org_units (id) on delete cascade,
  kind           text not null check (kind in ('department', 'level', 'year', 'term')),
  name_ar        text not null check (char_length(name_ar) between 1 and 200),
  name_en        text check (char_length(name_en) <= 200),
  sort_order     int not null default 0,
  is_active      boolean not null default true,
  created_at     timestamptz not null default now()
);
create index org_units_institution_idx on public.org_units (institution_id, parent_id);

create table public.subjects (
  id             uuid primary key default gen_random_uuid(),
  institution_id uuid not null references public.institutions (id) on delete cascade,
  unit_id        uuid references public.org_units (id) on delete set null,
  name_ar        text not null check (char_length(name_ar) between 1 and 200),
  name_en        text check (char_length(name_en) <= 200),
  is_active      boolean not null default true,
  created_at     timestamptz not null default now()
);
create index subjects_institution_idx on public.subjects (institution_id, unit_id);

alter table public.institutions enable row level security;
alter table public.org_units   enable row level security;
alter table public.subjects    enable row level security;

-- Signed-in users read active rows; admins read/write everything.
create policy "read active institutions" on public.institutions for select to authenticated
  using (is_active or public.is_admin());
create policy "admin writes institutions" on public.institutions for all to authenticated
  using (public.is_admin()) with check (public.is_admin());

create policy "read active org units" on public.org_units for select to authenticated
  using (is_active or public.is_admin());
create policy "admin writes org units" on public.org_units for all to authenticated
  using (public.is_admin()) with check (public.is_admin());

create policy "read active subjects" on public.subjects for select to authenticated
  using (is_active or public.is_admin());
create policy "admin writes subjects" on public.subjects for all to authenticated
  using (public.is_admin()) with check (public.is_admin());

-- ───────── audit log ─────────
create table public.audit_log (
  id          bigint generated always as identity primary key,
  actor_id    uuid,
  target_id   uuid,
  action      text not null,
  old_value   jsonb,
  new_value   jsonb,
  created_at  timestamptz not null default now()
);
alter table public.audit_log enable row level security;

create policy "admin reads audit log" on public.audit_log for select to authenticated
  using (public.is_admin());
-- No insert/update/delete policies: only the trigger below (security definer) writes.

create or replace function public.log_profile_change()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  if new.role is distinct from old.role
     or new.status is distinct from old.status then
    insert into public.audit_log (actor_id, target_id, action, old_value, new_value)
    values (
      auth.uid(), new.id, 'profile_role_status_changed',
      jsonb_build_object('role', old.role, 'status', old.status),
      jsonb_build_object('role', new.role, 'status', new.status, 'reason', new.status_reason)
    );
  end if;
  return new;
end;
$$;

create trigger log_profile_change
  after update on public.profiles
  for each row execute function public.log_profile_change();
