# AGENTS.md

Architecture reference for AI agents working on **Exameow**. Read this before exploring the codebase.

## What Exameow Is

AI-powered exam question generator. Users upload study materials (PDF, DOCX, XLSX, PPTX, EPUB, ODT, TXT, CSV, HTML) and get exam questions generated via any OpenAI-compatible API. Includes a built-in practice/quiz mode with wrong-question tracking. Exports to XLSX/CSV.

Version `1.2.1` (kept in sync across root `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `workers/package.json`).

## Release Rules

- **版本号语义（semver）**：第一位 = 不兼容的大更新；第二位 = 新功能；第三位 = Bug 修复。
- **Bump 版本时**：同步改 4 个文件 + `Cargo.lock` 中 `name = "exameow"` 条目（**只改 exameow 条目，千万别全局替换**——`cesu8` 等依赖锁版本也是 x.y.z，误改会导致全平台构建失败）。
- **发布流程**：bump 提交 → 打 `v*` tag 推送触发 CI（desktop/mobile/docker 三条流水线）→ CI 生成的 GitHub Release **默认是草稿，必须发布（`gh release edit vX.Y.Z --draft=false`），否则 Tauri 更新器看不到 `latest.json`** → 用 `bash scripts/deploy-cf.sh` 顺便更新 Cloudflare 线上版。
- **移动端 OTA 热更新**：`src-tauri/src/ota.rs` 自研实现（assets 替换 + 三态回滚 staged→booting→committed），仅 Android/iOS 生效，桌面端仍用官方 updater。CI 随 release 附加 `mobile-dist.tar.gz` + `mobile-ota.json`；App 查 `releases/latest/download/mobile-ota.json`。**若某版本前端依赖新增的原生能力（Rust 命令/插件），发版前必须把仓库根 `ota.json` 的 `minShell` 提高到能支持它的最低 APK 版本**，否则旧壳会热更到不兼容的前端，调新命令时报 `command xxx not found`（v1.3.5 真实事故：`explain_question` 新增但 minShell 滞留 1.3.0，旧壳热更后 AI 解析全挂；且已中招设备无法靠 OTA 自愈——minShell 只在下载决策时校验，必须重装新 APK）。纯前端修复无需动 `minShell`。**防忘**：mobile CI 首步跑 `scripts/check-ota-minshell.sh`，自动 diff 前端 `invoke()` 命令集与上一 tag 原生代码（`src-tauri/` + `plugins/`），发现新命令但 `minShell` 未提到 ≥ 当前版本则流水线直接失败。

**章节功能发布提醒**：`auto_chapter` / `chapter_names` 依赖本次新增的 Rust 生成提示词能力。下次发布此功能时须升级次版本，并将 `ota.json` 的 `minShell` 提高到包含该能力的新壳版本；旧 1.5.0 壳会忽略这些参数，不能仅靠新命令检测覆盖此兼容性变化。

## Learning Platform (accounts & roles, in progress)

Optional layer on top of Exameow (admin / teacher / student), **fully self-hosted in the Axum server** (no external service). Available on web/Docker builds only; Cloudflare and Tauri builds hide it unless `VITE_PLATFORM_API` points to a platform server. All existing features stay public (no account needed).
- Backend: `packages/server/src/platform.rs` — own SQLite file (`PLATFORM_DB_PATH`, default `./exameow-platform.db`; kept separate because `relay.rs` purges its DB after 7 days). Argon2 passwords, hashed Bearer session tokens (30 days), per-IP/per-email rate limits, server-side role whitelist (clients can only request `student`/`teacher`; teachers start `pending`). Routes: `/api/platform/{register,login,logout,me}`. Admin/tree logic in `platform_admin.rs`: `/api/platform/admin/{users,institutions,units,subjects}` (require active `admin`; admins cannot change their own role/status; role/status change or password reset revokes that user's sessions; every change goes to `audit_log`), plus read routes `/api/platform/institutions[/{id}/structure]` for any active user. Tree rules: `department > level > year > term` (child rank must exceed parent's; schools have no departments).
- First admin: set `PLATFORM_ADMIN_EMAIL` + `PLATFORM_ADMIN_PASSWORD` (created at startup only if no admin exists; no default credentials).
- Frontend: `lib/platformApi.ts`, `stores/auth.ts`, `views/platform/*`, strings in `i18n/platform.ts` (ar + en fallback). Routes opt in to guards via `meta.requiresAuth / requiresActive / roles / guestOnly` (see `router/index.ts`). Authorization for every future endpoint must call `platform::authenticate` server-side; never trust the client's role.
- `platform_learning.rs`: teacher↔subject assignments (teacher requests → admin approves in `/platform/admin/teaching`), student placement/enrollment, following teachers, teacher/subject public pages. Shared helpers (`require_active/require_role/require_admin`, `lock`, `text`, `audit`) live in `platform.rs`; each module exposes a `SCHEMA` const applied by `platform::apply_schema` (idempotent; new columns via `add_column_if_missing`).
- `platform_content.rs`: teacher content — posts (article/summary, 1 attachment ≤10MB, ext whitelist + magic-byte check, stored in `PLATFORM_FILES_DIR`), courses with ordered lessons (video URL → server computes `embed_url` for YouTube/Vimeo only; other https links stay external; never iframe user URLs), live sessions (https join link). Content is visible only while `published` AND the teacher is an active teacher with an *approved* assignment for that subject (see `visible()`); admins can only unpublish/cancel/delete (moderation), never rewrite. Text is rendered as plain text in the frontend — never `v-html`.
- `platform_engage.rs`: lesson progress (enrolled students only), reviews (1–5 + plain-text comment; only students enrolled in the course's/teacher's subject may review; shown by first name only; owner/admin can delete), in-app notifications (`notify`, `notify_audience` are called from admin/learning/content modules; stored as `kind`+`data` JSON and rendered client-side via `formatNotification` so they follow the UI language).
- `platform_exams.rs`: graded assessments. A teacher publishes a local question bank (`QuestionBank.questions`, produced by the generator) for an approved subject; enrolled students take it (`/assessments/{id}/start` returns questions WITHOUT answers/analysis; deadline, attempt limit and open/close window are enforced server-side; in-progress attempts resume). Objective types auto-grade via `relay::grade` + Arabic normalization (`norm_text`, `canon_tf`, `a|b` alternatives for fill-blank); short answers are graded by the teacher (`PATCH /attempts/{id}/grade`). Questions are frozen once attempts exist. Students can turn a reviewed assessment into a local practice bank (existing practice mode). Frontend: `views/platform/{AssessmentEditor,Assessment,TakeAssessment,Attempt,AssessmentResults}View.vue`.
- Bidi rule for the UI: wrap numbers/labels like `5 / 9` and `A.` in `dir="ltr"` inline-blocks; never put `dir="auto"` on a row that starts with a Latin label.
- **Identity (phase 1-1, azlearn)**: `platform_public.rs` serves public config/logo/legal (settings keys `brand.*`, `legal.*`; admin writes via `/api/platform/admin/{branding,branding/logo,legal/{slug}}`; brand colour must have ≥4.5:1 vs white). Frontend sets `data-brand="azlearn"` when the platform is enabled; the azlearn palette lives in `assets/main.css` (`[data-brand="azlearn"]` / `.dark[data-brand="azlearn"]`, extras `--azl-*`; teal is CTA-only and takes dark text `--azl-on-teal`; custom brand colour and user accent override `--md-primary`). `BrandMark.vue` shows the official lockup `frontend/public/azlearn-logo.png` (cropped, transparent, on a white chip so it reads in dark mode; favicon `azlearn-favicon.png`) until an admin sets a custom name/logo. The original 1 MB source PNG lives on `main` (>512 KB server upload limit). Registration requires recorded consent (`users.consented_at`).
- **Admin shell (phase 1-2)**: `/platform/admin` is a layout route (`components/platform/AdminLayout.vue`, sidebar with pending/report badges from the shared `lib/adminStats.ts`) whose children are the admin pages (URLs unchanged) plus `AdminOverviewView` (stats + "waiting for you"). `/platform` redirects admins there. `GET /admin/users` takes `limit` (default/max 200) and `offset`; the UI asks for page+1 rows to detect a next page. Note: `client_ip` trusts `X-Forwarded-For`, so per-IP rate limits assume a reverse proxy that overwrites it (harden in 1-10).
- **Settings & AI (phase 1-3)**: `platform_ai.rs` + `views/platform/AdminSettingsView.vue` (`/platform/admin/settings`). `GET/PUT /admin/settings` (AI endpoint/model/key, caps, `teachers.can_create_exams`; the API key is encrypted in `settings` as `ai.key`, **never returned** — only `key_saved`; `AI_*` env vars are the per-field fallback), `POST /admin/settings/ai/test` (accepts unsaved values; provider errors are scrubbed of the key), `GET /admin/ai/usage`, `POST /admin/ai/generate` (multipart `params` JSON [+ `file`]; reserves one call in `ai_usage` against the platform cap and the per-admin cap — 429 `ai_cap_platform|ai_cap_admin` with `resets_at` — before contacting the provider, and refunds it if the provider fails). Caps count requests per UTC day (default 200 platform / 50 per admin; 0 blocks). Branding/legal editing in the page reuses the 1-1 endpoints. `PlatformError.data` carries extra fields of an error body. `routes::extract_text` is the shared upload→text helper. The teacher-exams flag is stored only; the exam builder (1-5) must enforce it.
- **Question bank (phase 1-4)**: `platform_bank.rs` + `views/platform/AdminBankView.vue` (`/platform/admin/bank`, components `SubjectPicker`, `BankItemForm`, `BankImportPanel`). Table `question_bank_items` (per subject; `stem_norm` = Arabic-folded/whitespace-collapsed stem used for duplicate detection and search; CASCADE with the subject, `created_by` SET NULL). Admin-only endpoints: `GET/POST /admin/bank` (list with type/difficulty/chapter/tag/q/state filters + `limit`/`offset`, returns `{items,total}`; create returns `{item,duplicate_of}` — duplicates only warn), `PATCH /admin/bank/{id}`, `GET /admin/bank/facets?subject_id`, `POST /admin/bank/bulk` (`archive|restore|delete`, ≤500 ids), `POST /admin/bank/import` (≤500 raw items per request, per-item validation → `{created, skipped_duplicates, duplicates_kept, rejected[{index,error}]}`, 8 MB body limit; the UI sends browser banks (`exameow-banks`), CSV/XLSX parsed client-side by `importParser`, and 1-5 will send generation results). Answers are normalised on save (choice → sorted letters, true/false → `A`/`B` with default صحيح/خطأ options); `platform_exams::validate_question` / `norm_text` are shared. Exams must COPY questions (snapshot, 1-5) — never reference bank rows. There is no single-item DELETE route; the UI deletes through bulk.
- **Exam builder (phase 1-5)**: `platform_exam_admin.rs` + `views/platform/{AdminExamsView,AdminExamEditorView}.vue` (`/platform/admin/exams[/new|/:id/edit]`, components `ExamAiPanel`, `ExamBankPicker`, `ExamPreview`, utils `utils/examBuilder.ts`). Admin exams have `teacher_id` NULL and `created_by` = admin (all queries now LEFT JOIN the teacher; `TEACHER_OK` in `platform_exams.rs` is the shared visibility predicate; `AssessmentInfo.teacher_id` is `Option`). Endpoints (admin only): `GET/POST /admin/exams`, `GET/PATCH/DELETE /admin/exams/{id}` (`DELETE ?confirm_title=` is required when attempts exist), `POST /admin/exams/{id}/{publish|unpublish|close|reopen|archive|restore|duplicate}`. Lifecycle `draft → published → closed → archived`; `phase` = effective state (a published exam past `closes_at` reads as closed, nothing is written). PRD E6 freezing: once any attempt exists the questions/scores/duration/opens/shuffle are frozen (409 `has_attempts`), the closing time can only be extended and attempts only increased (409 `only_extend`). Admins can only moderate (close/archive/delete/duplicate) a teacher's exam, never edit it; a teacher cannot edit an exam an admin closed (409 `closed`). Questions are cleaned with `platform_bank::clean_exam_question` (answers canonicalised; errors carry `question_id`/`index`); `assessments.source_map` (qid → bank item id) is statistics only — exams are snapshots. Students can still open a closed exam's page and resume an in-progress attempt, but cannot start new ones. Admins grade attempts of any exam (`grade_handler` allows teacher|admin; approved-teacher grading of admin exams + the pending-grading page come in 1-7). `shuffle_*`, `pass_mark` and `release_mode` are enforced by the taking flow (phase 1-6, below).
- **Exam taking (phase 1-6)**, all in `platform_exams.rs` (`taking_tests` module): (1) **Per-attempt shuffling** — `make_order` deals question order and, for choice questions only, an option permutation when the exam has `shuffle_questions`/`shuffle_options`; stored in `attempts.order_json` (`Order {q, o}`), reused on resume; true/false options are never permuted. Students answer in *displayed* letters; `to_original` maps them back to original letters before grading and the stored `answers` of a submitted attempt are in original letters (while in progress `answers` holds the autosave in displayed letters). `relay::normalize_choice` now accepts letters A–J. (2) **Autosave** `PUT /attempts/{id}/answers` (student only, 30/min/attempt, `saved_at`) and `StartRes.saved_answers` for resuming on any device; the frontend debounces 5 s + 30 s sweep and keeps a local draft that wins per question. (3) **Overdue attempts** are settled (`settle_if_due`): submitted from the autosave at the deadline (`submitted_at` = deadline) if anything was ever saved, else `expired` with 0; triggered on start/save/submit, on `results_summary`, and hourly by `settle_overdue` in `main.rs`. (4) **Integrity events** `POST /attempts/{id}/events {type:'tab_leave'}` → `attempts.tab_leaves` (cap 1000, ignored after the attempt ends; informational only; shown to graders/admins, never to the student; a page reload counts as leaving). (5) **Release timing** — `release_mode='after_close'` makes the server withhold score/pending/items/pass flag from the student (`AttemptResult.released=false` + `release_at`, also in `MyAttempt`) until the exam is closed (status closed/archived or `closes_at` passed); graders always see everything. Availability *notifications* are phase 1-8. (6) **Pass mark** — `passed` = score/total ≥ `pass_mark` % (inclusive), `None` while short answers are pending or without a pass mark; the results summary counts passers. Load test: `scripts/loadtest-exam.cjs` (release build; 300 students, 40 questions).
- **Grading, results & export (phase 1-7)**: `platform_grading.rs` (+ `platform_exams::can_grade`), frontend `views/platform/{GradingQueue,GradeExam,AssessmentResults}View.vue`, `api/platformGrading.ts`. **Who grades / sees results** (`can_grade`): any admin; the owning teacher of a teacher's exam; for an admin exam (no owner) any active teacher *approved* for its subject (PRD G2) — a colleague approved for the same subject does NOT get a teacher's own exam. Endpoints (all `require_active`, then `can_grade` → 403): `GET /grading/pending` (admin/teacher; queue of exams with ungraded written answers, oldest first), `GET /assessments/{id}/grading?pending=` (quick-grading sheet: every *written* short answer grouped by question; blank answers are auto-0 and not listed), `POST /assessments/{id}/grade-batch` (≤500 `{attempt_id, question_id, points}`, ONE transaction, answers of another exam → 404, duplicates → 400), `GET /assessments/{id}/analytics`, `GET /assessments/{id}/attempts` (students table: whitelisted `sort`/`dir`, `result` filter, name search, paging; **e-mail only for admins**), `GET /assessments/{id}/export?format=xlsx|csv&part=results|questions&lang=ar|en` (10/min/user, audited, ASCII file name, `no-store`). `grade_attempt` now lets a grader *correct* a manual grade (only written ShortAnswer answers; objective questions and blank answers stay untouchable) and notifies the student only when the last pending answer gets its first grade. Analytics rules: score statistics (average/median/high/low/distribution/pass-fail) use each student's best **fully graded** attempt; students whose attempts all wait for grading are `awaiting_grading`; distribution = five 20 % bins (100 % goes in the last, a boundary goes up); question `rate` = points earned ÷ available over *all* graded outcomes; `weak` (<30 %) / `easy` (>90 %) only with ≥5 graded answers. `exameow-core` gained a generic table export (`export::{Cell, Sheet, export_tables_xlsx, export_table_csv}`; multi-sheet, typed cells, RTL, frozen header; CSV has a UTF-8 BOM and prefixes a leading `= + - @ TAB CR` with `'` — XLSX stores text as shared strings so it can never become a formula). `Stats.pending_grading` feeds the admin sidebar badge and the teacher home link.
- `platform_ops.rs`: content reports (unique open report per user/target, ≥3 distinct reporters auto-hide published content, admin closes all open reports of a target at once), admin stats, audit viewer (cursor pagination), unified search (visible content only, wildcard-escaped), self-service password change (revokes other sessions) and account deletion (cascades; last admin protected). Security headers (`nosniff`, referrer, frame) are set in `main.rs`.
- **Exam-platform PRD** (`docs/PRD_exam_platform_ar.md`, force-added because `docs/` is gitignored) drives the next work. Phase **1-0 done**: `platform_settings.rs` (encrypted `settings` table: `Crypto` AES-256-GCM, ciphertext bound to the setting name via AAD, master key file auto-created 0600 and never overwritten) and the **assessments migration** (`platform_exams::migrate`: atomic rebuild of `assessments` with FKs off + `foreign_key_check`, adds `created_by`, nullable `teacher_id`, `shuffle_*`, `pass_mark`, `release_mode`, `closed/archived` states; `attempts.saved_at/tab_leaves/order_json`; `users.consented_at`). Migration tests run against `packages/server/tests/fixtures/platform_v1.sql` (a real DB dump from the pre-1-0 server); regenerate such fixtures BEFORE changing a schema again. `assessments.teacher_id` is NULL for admin-created exams (handled since 1-5).
- All platform phases (0–6) done and tested. Full user/ops documentation: `docs/PLATFORM_ar.md`. Possible next steps: SMTP (email verification / password reset), PostgreSQL for large deployments, institution-admin role scoping, payments for paid courses.
- E2E note: headless Chromium names blob downloads with Arabic filenames "download"; ASCII names are fine.

## Tech Stack

- **Frontend**: Vue 3 + Vite + Pinia + Vue Router + TypeScript, Tailwind CSS 3.4 (custom Material You tonal palette)
- **Desktop/Mobile**: Tauri v2 (Rust shell)
- **Self-hosted backend**: Rust / Axum 0.8
- **Serverless backend**: Cloudflare Workers (Hono 4.7)
- **Shared core logic**: Rust crate `exameow-core` (parsing, AI client, exam gen, export, encrypted config)
- **Package mgmt**: pnpm workspace + Cargo workspace (monorepo)

## Three-Backend Architecture (KEY CONCEPT)

The **same Vue frontend** targets three interchangeable backends. `frontend/src/api/index.ts` auto-detects the platform at runtime and routes accordingly:

| Platform | Detector | API module | Backend |
|----------|----------|-----------|---------|
| Tauri desktop/mobile | `isTauri()` | `api/bridge.ts` → `invoke()` | `src-tauri/src/lib.rs` (Tauri commands) |
| Cloudflare | `isCloudflare()` | `api/cf.ts` → `fetch()` | `workers/src/index.ts` (Hono) |
| Web / Docker | fallback | `api/http.ts` → `fetch()` | `packages/server` (Axum) |

Platform detection lives in `frontend/src/utils/platform.ts`.

**Important consequence**: Core logic (file parsing, prompt building, export) is **duplicated** in Rust (`packages/core`) and TypeScript (`workers/src/*`, `frontend/src/utils/*`). When changing generation prompts, parsing, or export format, update BOTH the Rust and TS implementations to keep parity.

## Directory Map

```
frontend/              Vue 3 SPA (hash routing)
  src/
    api/               Platform-routed API layer (index.ts dispatches to bridge/http/cf)
    stores/            Pinia: exam.ts, practice.ts, config.ts, wrongQuestions.ts, fileInput.ts, i18n.ts
    views/             GenerateView, PracticeView, ConfigView, PreviewView
    utils/             Browser-side: fileParser, pdfParser, importParser, aiClient, platform
    components/        config/ generate/ layout/ practice/ preview/
  dist/                Build output (served by Axum or copied to workers/public)

packages/
  core/                Rust crate `exameow-core` (shared server logic)
    src/parser/        parse_file() dispatch → pdf/docx/pptx/excel/csv/epub/odt/html/txt
    src/ai/            OpenAI-compatible HTTP client (client.rs)
    src/exam/          types.rs (Question/ExamParams), prompt.rs (generate_exam + prompts)
    src/export/        writer.rs (CSV), xlsx.rs (manual ZIP+XML, no lib)
    src/config/        store.rs (AES-256-GCM encrypted config persistence)
  server/              Axum HTTP server; routes.rs (AI 端点) + relay.rs (在线考试,SQLite via rusqlite)
  shared/              TS shared types (@exameow/shared) src/types.ts

src-tauri/             Tauri app; src/lib.rs = all Tauri commands; tauri.conf.json; capabilities/
workers/               Cloudflare Worker; src/{index,ai,exam,parser,export,types,relay}.ts; wrangler.toml; migrations/ (D1)
                       relay.ts = exam publish/take relay (D1 EXAM_DB, cron cleanup; /api/exam/* routes)
scripts/               deploy-cf.sh, docker-build.sh, start-android-emulator.sh, check-ota-minshell.sh
.github/workflows/     release-desktop.yml / release-mobile.yml / release-docker.yml (v* tag 触发;Docker 推 Docker Hub `ailm32442/exameow`)
```

## Commands

| Task | Command |
|------|---------|
| Install deps | `pnpm install` |
| Frontend dev | `cd frontend && pnpm dev` (port 5273) |
| Frontend build | `cd frontend && pnpm build` |
| Frontend typecheck | `cd frontend && pnpm run type-check` |
| Axum server | `cargo run -p exameow-server` (port 3000) |
| Tauri dev | `pnpm tauri dev` |
| Tauri build | `pnpm tauri build` |
| CF Worker dev | `cd workers && pnpm dev` |
| CF Worker typecheck | `cd workers && pnpm typecheck` |
| CF deploy | `bash scripts/deploy-cf.sh` |
| Docker | `docker compose up -d --build` |
| Quick launcher | `./start.sh` (macOS/Linux) / `start.bat` |

**No test suite exists. No ESLint/rustfmt/clippy config files.** Use `pnpm run type-check` (frontend), `pnpm typecheck` (workers), and `cargo build` for verification.

## Data Models

Defined in `packages/shared/src/types.ts` (TS) and `packages/core/src/exam/types.rs` (Rust) — keep in sync.

- **Question**: `{ id, type, stem, options[], answer, analysis }`
- **QuestionType**: `SingleChoice | MultiChoice | TrueFalse | FillBlank | ShortAnswer`
- **Difficulty**: `Easy | Medium | Hard`
- **ExamParams**: `{ question_types, count, type_counts?, difficulty, language, topic_filter?, text?, batch_index?, batch_total?, source_name? }`
- **QuestionBank**: `{ id, name, questions[], createdAt, source }`
- **PracticeSession**: `{ bankId, mode, questions[], currentIndex, startedAt, finishedAt?, mockConfig? }`
- **WrongQuestionEntry**: `{ questionId, wrongCount, consecutiveCorrect, lastWrongAt, addedAt }`
- **AIConfig**: `{ endpoint, api_key, model }`

## Storage (no database)

- **Browser `localStorage`**: question banks, practice sessions, wrong questions, config. Keys: `exameow-banks`, `exameow-practice-session`, `exameow-wrong`, `exameow-questions`, `exameow-sourcefile`.
- **Native (Tauri/Axum)**: AI credentials encrypted with AES-256-GCM via `ConfigStore` in OS config dir (macOS `~/Library/Application Support/Exameow/`, Linux `~/.config/Exameow/`, Windows `%APPDATA%/Exameow/`).
- **Server is stateless for AI** — 但自 v1.3 起内置在线考试 relay(SQLite),Docker 版完全自包含,不依赖演示站。反滥用:每 IP 每日发布限 20 场;≥3 个独立 IP 举报自动暂停;管理员页 `#/admin`(CF 密钥存 `wrangler secret`,本地备份于 gitignored 的 `.secrets/`)。

## Environment Variables

| Var | Default | Used by | Notes |
|-----|---------|---------|-------|
| `AI_ENDPOINT` | `https://api.openai.com/v1` | Server | OpenAI-compatible endpoint |
| `AI_API_KEY` | (required) | Server | Provider API key |
| `AI_MODEL` | `gpt-4o` | Server | Default model |
| `PORT` | `3000` | Server | Axum port |
| `STATIC_DIR` | `../frontend/dist` (local) | Server | Built frontend path |
| `ADMIN_TOKEN` | `pass` | Server | Docker 管理员密钥；`pass` 时管理员页强制修改,改后写入 `ADMIN_TOKEN_FILE` |
| `EXAM_DB_PATH` | `./exameow.db` | Server | 在线考试 SQLite 路径(docker-compose 挂卷 `/app/data`) |
| `ADMIN_TOKEN_FILE` | `./admin_token.txt` | Server | 修改后的密钥持久化文件 |
| `PLATFORM_DB_PATH` | `./exameow-platform.db` | Server | 教学平台账号/机构数据库(docker-compose 挂卷 `/app/data`) |
| `PLATFORM_FILES_DIR` | `./platform-files` | Server | 教学平台附件目录(docker-compose 挂卷 `/app/data/files`) |
| `PLATFORM_KEY_FILE` | `<db dir>/platform.key` | Server | 平台主密钥(AES-256-GCM,首次启动自动生成,权限 0600;**必须随数据库一起备份**,丢失后只需重新填写 AI Key/SMTP) |
| `PLATFORM_ADMIN_EMAIL` / `PLATFORM_ADMIN_PASSWORD` | — | Server | 首个平台管理员(仅当尚无管理员时创建) |
| `VITE_PLATFORM_API` | — | Frontend | 平台服务地址;Tauri/CF 构建需要它才会启用账号功能 |
| `VITE_EXAM_RELAY` | — | Frontend | 覆盖考试中转地址;默认 Tauri 用 CF 域名,网页/Docker 走同源 |
| `VITE_CLOUDFLARE` | — | Frontend | Set in deploy-cf.sh to trigger CF routing |
| `CF_ACCOUNT_ID` / `CF_API_TOKEN` | wrangler.toml | Workers | CF model listing |

## Data Flow (Generate)

1. Upload in `GenerateView` → `stores/exam.ts` calls `api.generateExam()`
2. `api/index.ts` routes by platform (Tauri invoke / Axum fetch / CF fetch)
3. Backend parses file → text (`parser`), builds prompt (`exam/prompt`), calls AI client
4. Returns JSON question array → parsed → back to frontend
5. `stores/exam.ts` persists to localStorage; `stores/practice.ts` creates a `QuestionBank`

**Batch/chunking**: Large text split into ~32K-char chunks (preserving markdown/table/section structure); ≤15 questions per batch, distributed proportionally. Multiple sequential AI calls per generation.

## Conventions / Gotchas

- **Rust ⇄ TS parity**: prompts, parsers, and XLSX export exist in both languages — change both.
- **CF Worker 调用最小化**: Cloudflare Workers 按请求计费且有免费额度限制。新功能优先用纯前端实现（localStorage/sessionStorage/路由状态），不得新增非必要的 `/api/*` 请求、轮询或重复拉取；确需后端时合并请求、利用缓存，避免每次交互都触发 Worker 调用。
- **XLSX has no library**: built manually as ZIP+XML in both `packages/core/src/export/xlsx.rs` and `workers/src/export.ts`.
- **Hash-based SPA routing** (`createWebHashHistory`); CF Worker + Axum both serve SPA fallback.
- **Material You theme** in `frontend/tailwind.config.js` (full tonal palette + custom animations).
- Docs: `README.md` (EN), `README_zh.md` (ZH). `frontend/README.md` is boilerplate.
- CI releases on `v*` tags: Tauri (linux/windows/macOS, x86_64+aarch64) + Docker to Docker Hub (`<DOCKERHUB_USERNAME>/exameow`).
