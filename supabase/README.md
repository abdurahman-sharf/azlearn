# Platform backend (Supabase)

Phase 0 schema for the education platform (admin / teacher / student).

## Setup

1. Create a Supabase project, then apply `migrations/*.sql` in order
   (SQL editor, or `supabase db push` with the Supabase CLI).
2. In Authentication → Providers → Email, keep **email confirmation enabled**.
3. Add to `frontend/.env.local` (never commit it):

   ```
   VITE_SUPABASE_URL=https://<project>.supabase.co
   VITE_SUPABASE_ANON_KEY=<publishable/anon key>
   ```

   Without these variables the platform UI stays hidden and the app behaves exactly as before.

## Creating the first admin

Admins cannot be created from the app. Sign up normally, then run in the SQL editor:

```sql
update public.profiles set role = 'admin', status = 'active'
where id = (select id from auth.users where email = 'you@example.com');
```

## Security model

- Sign-up metadata can only request `student` or `teacher`; teachers start `pending`.
- `guard_profile_update` trigger + RLS stop users changing their own `role`/`status`.
- All role/status changes are written to `audit_log` (admin-readable only).
- Never ship the `service_role` key to the frontend.
