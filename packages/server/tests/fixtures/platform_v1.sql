-- Platform DB produced by the pre-phase-1-0 server (real API-seeded data) for migration tests.
-- All accounts use the password 'password123' (admin: boss@x.com / bosspass123).
BEGIN TRANSACTION;
CREATE TABLE assessments (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  questions TEXT NOT NULL,
  question_count INTEGER NOT NULL,
  total_points REAL NOT NULL,
  duration_min INTEGER,
  opens_at INTEGER,
  closes_at INTEGER,
  max_attempts INTEGER NOT NULL DEFAULT 1,
  show_answers INTEGER NOT NULL DEFAULT 1,
  status TEXT NOT NULL CHECK (status IN ('draft','published')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
INSERT INTO "assessments" VALUES('40c5a32e-c1c6-4c7f-a7d5-af08dad60be1','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','f103bb27-9f87-4655-93d5-2471831605ce','اختبار البرمجة','اختبار نموذجي','[{"id":"q1","type":"single_choice","stem":"عاصمة مصر؟","options":["الخرطوم","القاهرة","صنعاء"],"answer":"B","analysis":"القاهرة"},{"id":"q2","type":"multi_choice","stem":"اختر العواصم","options":["القاهرة","جدة","صنعاء"],"answer":"AC","analysis":"","score":2.0},{"id":"q3","type":"true_false","stem":"الأرض كروية","options":[],"answer":"صح","analysis":""},{"id":"q4","type":"fill_blank","stem":"عاصمة مصر ____","options":[],"answer":"القاهرة|مصر","analysis":""},{"id":"q5","type":"short_answer","stem":"اشرح المتغير","options":[],"answer":"نموذج","analysis":"","score":4.0}]',5,9.0,30,NULL,NULL,3,1,'published',1700000000000,1700000000000);
INSERT INTO "assessments" VALUES('3557a10a-6d08-4fa4-ae07-f9ddb810d016','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','f103bb27-9f87-4655-93d5-2471831605ce','مسودة اختبار',NULL,'[{"id":"q1","type":"single_choice","stem":"عاصمة مصر؟","options":["الخرطوم","القاهرة","صنعاء"],"answer":"B","analysis":"القاهرة"},{"id":"q2","type":"multi_choice","stem":"اختر العواصم","options":["القاهرة","جدة","صنعاء"],"answer":"AC","analysis":"","score":2.0}]',2,3.0,NULL,NULL,NULL,1,0,'draft',1700000000000,1700000000000);
CREATE TABLE attempts (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  started_at INTEGER NOT NULL,
  submitted_at INTEGER,
  status TEXT NOT NULL CHECK (status IN ('in_progress','submitted','expired')),
  answers TEXT NOT NULL DEFAULT '{}',
  results TEXT NOT NULL DEFAULT '[]',
  score REAL NOT NULL DEFAULT 0,
  pending INTEGER NOT NULL DEFAULT 0
);
INSERT INTO "attempts" VALUES('7cf22259-5901-4b3d-b7e8-a91185bdb3c0','40c5a32e-c1c6-4c7f-a7d5-af08dad60be1','54953074-4328-405f-a844-cd296c9646e4',1791395211336,1791395211341,'submitted','{"q2":"AC","q3":"صحيح","q5":"إجابتي","q4":"مِصْر","q1":"B"}','[{"id":"q1","correct":true,"points":1.0,"max":1.0},{"id":"q2","correct":true,"points":2.0,"max":2.0},{"id":"q3","correct":true,"points":1.0,"max":1.0},{"id":"q4","correct":true,"points":1.0,"max":1.0},{"id":"q5","correct":false,"points":3.0,"max":4.0}]',8.0,0);
INSERT INTO "attempts" VALUES('af7ee9ff-bc19-4403-814b-f23d17a37a5b','40c5a32e-c1c6-4c7f-a7d5-af08dad60be1','e19c22a2-8340-4b82-a6f2-6b7d2bee761c',1791395211350,1791395211354,'submitted','{"q1":"A","q5":"شرح"}','[{"id":"q1","correct":false,"points":0.0,"max":1.0},{"id":"q2","correct":false,"points":0.0,"max":2.0},{"id":"q3","correct":false,"points":0.0,"max":1.0},{"id":"q4","correct":false,"points":0.0,"max":1.0},{"id":"q5","correct":null,"points":0.0,"max":4.0}]',0.0,1);
INSERT INTO "attempts" VALUES('3e18b98f-08ca-45f8-9ef1-a533c6c84f36','40c5a32e-c1c6-4c7f-a7d5-af08dad60be1','54953074-4328-405f-a844-cd296c9646e4',1791395211358,NULL,'in_progress','{}','[]',0.0,0);
CREATE TABLE audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  actor_id TEXT,
  target_id TEXT,
  action TEXT NOT NULL,
  detail TEXT,
  created_at INTEGER NOT NULL
);
INSERT INTO "audit_log" VALUES(1,'308b5097-0e89-4ec5-b59a-543de4121ec3','651bbbbd-1032-4402-919f-87e69142f61f','institution_created','جامعة صنعاء',1791395207845);
INSERT INTO "audit_log" VALUES(2,'308b5097-0e89-4ec5-b59a-543de4121ec3','0dc6193b-d942-4334-861d-21c1bb4e3763','unit_created','department: علوم الحاسوب',1791395207853);
INSERT INTO "audit_log" VALUES(3,'308b5097-0e89-4ec5-b59a-543de4121ec3','b2368159-ab01-4600-b56a-5905577f3bbf','unit_created','level: المستوى الأول',1791395207858);
INSERT INTO "audit_log" VALUES(4,'308b5097-0e89-4ec5-b59a-543de4121ec3','f103bb27-9f87-4655-93d5-2471831605ce','subject_created','برمجة 1',1791395207863);
INSERT INTO "audit_log" VALUES(5,'308b5097-0e89-4ec5-b59a-543de4121ec3','ab234b3f-bedc-4b8f-9ba4-4c4cfe1a1867','institution_created','مدرسة النور',1791395207868);
INSERT INTO "audit_log" VALUES(6,'308b5097-0e89-4ec5-b59a-543de4121ec3','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','user_role_status_changed','teacher/pending -> teacher/active',1791395210533);
INSERT INTO "audit_log" VALUES(7,'308b5097-0e89-4ec5-b59a-543de4121ec3','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','teaching_decided','f103bb27-9f87-4655-93d5-2471831605ce -> approved',1791395211277);
CREATE TABLE courses (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  status TEXT NOT NULL CHECK (status IN ('draft','published')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
INSERT INTO "courses" VALUES('9993122b-bc39-44e9-b832-85e5d6e7e293','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','f103bb27-9f87-4655-93d5-2471831605ce','دورة بايثون',NULL,'published',1791395211295,1791395211304);
CREATE TABLE files (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  original_name TEXT NOT NULL,
  mime TEXT NOT NULL,
  size INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE institutions (
  id TEXT PRIMARY KEY,
  type TEXT NOT NULL CHECK (type IN ('school','institute','university')),
  name_ar TEXT NOT NULL,
  name_en TEXT,
  city TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
INSERT INTO "institutions" VALUES('651bbbbd-1032-4402-919f-87e69142f61f','university','جامعة صنعاء','Sanaa University','صنعاء',1,1791395207844);
INSERT INTO "institutions" VALUES('ab234b3f-bedc-4b8f-9ba4-4c4cfe1a1867','school','مدرسة النور',NULL,NULL,1,1791395207868);
CREATE TABLE lesson_progress (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  lesson_id TEXT NOT NULL REFERENCES lessons(id) ON DELETE CASCADE,
  completed_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, lesson_id)
);
INSERT INTO "lesson_progress" VALUES('54953074-4328-405f-a844-cd296c9646e4','684197df-e5b4-4640-8997-6a7e94c9a528',1791395211307);
CREATE TABLE lessons (
  id TEXT PRIMARY KEY,
  course_id TEXT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  section TEXT,
  title TEXT NOT NULL,
  description TEXT,
  video_url TEXT NOT NULL,
  embed_url TEXT,
  video_kind TEXT NOT NULL CHECK (video_kind IN ('youtube','vimeo','file','link')),
  created_at INTEGER NOT NULL
);
INSERT INTO "lessons" VALUES('684197df-e5b4-4640-8997-6a7e94c9a528','9993122b-bc39-44e9-b832-85e5d6e7e293',1,NULL,'الدرس 1',NULL,'https://youtu.be/dQw4w9WgXcQ','https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ','youtube',1791395211302);
CREATE TABLE live_sessions (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  starts_at INTEGER NOT NULL,
  duration_min INTEGER NOT NULL,
  join_url TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('scheduled','cancelled')),
  created_at INTEGER NOT NULL
);
INSERT INTO "live_sessions" VALUES('cff9ce87-ecb4-40e3-a516-60856337ed72','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','f103bb27-9f87-4655-93d5-2471831605ce','مراجعة',NULL,1791481611309,60,'https://meet.example.com/x','scheduled',1791395211311);
CREATE TABLE notifications (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  data TEXT NOT NULL,
  link TEXT,
  read_at INTEGER,
  created_at INTEGER NOT NULL
);
INSERT INTO "notifications" VALUES('cee034e1-f6a2-4c41-b73e-a7acf4b712df','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','account_active','{"reason":null}','/platform',NULL,1791395210534);
INSERT INTO "notifications" VALUES('6fc9e5e0-5f27-4b7d-aba0-b21a3fd8dd78','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','teaching_approved','{"subject":"برمجة 1"}','/platform/subjects/f103bb27-9f87-4655-93d5-2471831605ce',NULL,1791395211278);
INSERT INTO "notifications" VALUES('101fa5b1a08242fd112b41777bd64f35','54953074-4328-405f-a844-cd296c9646e4','new_post','{"teacher":"د. خالد","title":"ملخص الفصل الأول"}','/platform/posts/bf87321e-e278-4298-b139-9e7eb3bd254b',NULL,1791395211292);
INSERT INTO "notifications" VALUES('688b9ecb47cdd52f9c4264ad0ea9da2d','e19c22a2-8340-4b82-a6f2-6b7d2bee761c','new_post','{"teacher":"د. خالد","title":"ملخص الفصل الأول"}','/platform/posts/bf87321e-e278-4298-b139-9e7eb3bd254b',NULL,1791395211292);
INSERT INTO "notifications" VALUES('cce25e191082a36084670fc8ba2a77e0','54953074-4328-405f-a844-cd296c9646e4','new_course','{"teacher":"د. خالد","title":"دورة بايثون"}','/platform/courses/9993122b-bc39-44e9-b832-85e5d6e7e293',NULL,1791395211297);
INSERT INTO "notifications" VALUES('29a4e05d48ed3a3c98f031b3ec89cc2e','e19c22a2-8340-4b82-a6f2-6b7d2bee761c','new_course','{"teacher":"د. خالد","title":"دورة بايثون"}','/platform/courses/9993122b-bc39-44e9-b832-85e5d6e7e293',NULL,1791395211297);
INSERT INTO "notifications" VALUES('8e6559e37ad0fbfe6cd7d6329aa0950b','54953074-4328-405f-a844-cd296c9646e4','live_scheduled','{"teacher":"د. خالد","title":"مراجعة"}','/platform/subjects/f103bb27-9f87-4655-93d5-2471831605ce',NULL,1791395211313);
INSERT INTO "notifications" VALUES('4902ddfa0f6cfcfca3c05be22eccb6ba','e19c22a2-8340-4b82-a6f2-6b7d2bee761c','live_scheduled','{"teacher":"د. خالد","title":"مراجعة"}','/platform/subjects/f103bb27-9f87-4655-93d5-2471831605ce',NULL,1791395211313);
INSERT INTO "notifications" VALUES('f45a8060-4a0f-4f5b-b7a8-a1ea31429bde','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','new_review','{"rating":5,"target":"course"}','/platform/courses/9993122b-bc39-44e9-b832-85e5d6e7e293',NULL,1791395211317);
INSERT INTO "notifications" VALUES('bbf1fe784111f96ca5423a6c8c154461','54953074-4328-405f-a844-cd296c9646e4','new_assessment','{"teacher":"د. خالد","title":"اختبار البرمجة"}','/platform/assessments/40c5a32e-c1c6-4c7f-a7d5-af08dad60be1',NULL,1791395211329);
INSERT INTO "notifications" VALUES('7f234900c0e8f1a83cea72d2392b1362','e19c22a2-8340-4b82-a6f2-6b7d2bee761c','new_assessment','{"teacher":"د. خالد","title":"اختبار البرمجة"}','/platform/assessments/40c5a32e-c1c6-4c7f-a7d5-af08dad60be1',NULL,1791395211329);
INSERT INTO "notifications" VALUES('7c0dacac-bd21-4c6a-a6b5-f6e8db3dbad2','54953074-4328-405f-a844-cd296c9646e4','assessment_graded','{"title":"اختبار البرمجة"}','/platform/attempts/7cf22259-5901-4b3d-b7e8-a91185bdb3c0',NULL,1791395211347);
CREATE TABLE org_units (
  id TEXT PRIMARY KEY,
  institution_id TEXT NOT NULL REFERENCES institutions(id) ON DELETE CASCADE,
  parent_id TEXT REFERENCES org_units(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('department','level','year','term')),
  name_ar TEXT NOT NULL,
  name_en TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
INSERT INTO "org_units" VALUES('0dc6193b-d942-4334-861d-21c1bb4e3763','651bbbbd-1032-4402-919f-87e69142f61f',NULL,'department','علوم الحاسوب',NULL,0,1,1791395207852);
INSERT INTO "org_units" VALUES('b2368159-ab01-4600-b56a-5905577f3bbf','651bbbbd-1032-4402-919f-87e69142f61f','0dc6193b-d942-4334-861d-21c1bb4e3763','level','المستوى الأول',NULL,0,1,1791395207857);
CREATE TABLE posts (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('article','summary')),
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('draft','published')),
  file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
INSERT INTO "posts" VALUES('bf87321e-e278-4298-b139-9e7eb3bd254b','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','f103bb27-9f87-4655-93d5-2471831605ce','summary','ملخص الفصل الأول','محتوى الملخص','published',NULL,1791395211291,1791395211291);
CREATE TABLE rate_limits (
  key TEXT NOT NULL,
  bucket INTEGER NOT NULL,
  count INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (key, bucket)
);
CREATE TABLE reports (
  id TEXT PRIMARY KEY,
  reporter_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_type TEXT NOT NULL CHECK (target_type IN ('post','course','live','assessment','review','teacher')),
  target_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('open','resolved','dismissed')),
  note TEXT,
  created_at INTEGER NOT NULL,
  resolved_by TEXT,
  resolved_at INTEGER
);
INSERT INTO "reports" VALUES('f9305cc8-742c-4442-9f8b-5be91749bcad','e19c22a2-8340-4b82-a6f2-6b7d2bee761c','post','bf87321e-e278-4298-b139-9e7eb3bd254b','اختبار','open',NULL,1791395211322,NULL,NULL);
CREATE TABLE reviews (
  id TEXT PRIMARY KEY,
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_type TEXT NOT NULL CHECK (target_type IN ('course','teacher')),
  target_id TEXT NOT NULL,
  rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
  comment TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (student_id, target_type, target_id)
);
INSERT INTO "reviews" VALUES('66b1cbc4-68fa-4f56-8408-97a59cb4195b','54953074-4328-405f-a844-cd296c9646e4','course','9993122b-bc39-44e9-b832-85e5d6e7e293',5,'ممتازة',1791395211316,1791395211316);
CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE student_placement (
  student_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  institution_id TEXT NOT NULL REFERENCES institutions(id) ON DELETE CASCADE,
  unit_id TEXT REFERENCES org_units(id) ON DELETE SET NULL
);
INSERT INTO "student_placement" VALUES('54953074-4328-405f-a844-cd296c9646e4','651bbbbd-1032-4402-919f-87e69142f61f','b2368159-ab01-4600-b56a-5905577f3bbf');
CREATE TABLE subject_enrollments (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, subject_id)
);
INSERT INTO "subject_enrollments" VALUES('54953074-4328-405f-a844-cd296c9646e4','f103bb27-9f87-4655-93d5-2471831605ce',1791395211280);
INSERT INTO "subject_enrollments" VALUES('e19c22a2-8340-4b82-a6f2-6b7d2bee761c','f103bb27-9f87-4655-93d5-2471831605ce',1791395211283);
CREATE TABLE subjects (
  id TEXT PRIMARY KEY,
  institution_id TEXT NOT NULL REFERENCES institutions(id) ON DELETE CASCADE,
  unit_id TEXT REFERENCES org_units(id) ON DELETE SET NULL,
  name_ar TEXT NOT NULL,
  name_en TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
INSERT INTO "subjects" VALUES('f103bb27-9f87-4655-93d5-2471831605ce','651bbbbd-1032-4402-919f-87e69142f61f','b2368159-ab01-4600-b56a-5905577f3bbf','برمجة 1','Programming 1',1,1791395207862);
CREATE TABLE teacher_follows (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, teacher_id)
);
INSERT INTO "teacher_follows" VALUES('54953074-4328-405f-a844-cd296c9646e4','ec78ba8a-d661-4fe7-949a-e19a7d0d20f4',1791395211285);
CREATE TABLE teacher_subjects (
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected')),
  created_at INTEGER NOT NULL,
  decided_at INTEGER,
  PRIMARY KEY (teacher_id, subject_id)
);
INSERT INTO "teacher_subjects" VALUES('ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','f103bb27-9f87-4655-93d5-2471831605ce','approved',1791395211272,1791395211276);
CREATE TABLE users (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  full_name TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('admin','institution_admin','moderator','teacher','student')),
  institution_type TEXT CHECK (institution_type IN ('school','institute','university')),
  status TEXT NOT NULL CHECK (status IN ('active','pending','rejected','suspended')),
  status_reason TEXT,
  created_at INTEGER NOT NULL
, bio TEXT);
INSERT INTO "users" VALUES('308b5097-0e89-4ec5-b59a-543de4121ec3','boss@x.com','$argon2id$v=19$m=19456,t=2,p=1$qDBQM2Vc3pdDVyPb1wf63w$p+mBfl5yINe2VP31oaODMxeGu7ov3j3QPXMg+P6LX5Q','Admin','admin',NULL,'active',NULL,1791395206362,NULL);
INSERT INTO "users" VALUES('ec78ba8a-d661-4fe7-949a-e19a7d0d20f4','t1@x.com','$argon2id$v=19$m=19456,t=2,p=1$IQhW9BeVnwqIbyPXpICFPw$45mviszf3Ibi5xQC3+Zr76XW78z1x2puCcJ0G9Vp5xY','د. خالد','teacher','university','active',NULL,1791395208446,NULL);
INSERT INTO "users" VALUES('de2902b0-0971-47e9-b7f1-b9c0f2d4c5ed','t2@x.com','$argon2id$v=19$m=19456,t=2,p=1$+v8dPjiqyY9ZSIHUNVzxEA$uVyZYqhcq6qQnbRg9xqZGXaLyAnvlWzzLhW/d64N0J8','معلم معلّق','teacher','university','pending',NULL,1791395208968,NULL);
INSERT INTO "users" VALUES('54953074-4328-405f-a844-cd296c9646e4','s1@x.com','$argon2id$v=19$m=19456,t=2,p=1$SD0sNPWpDvM6s1t19DXzOQ$mHJG6vnEOr3Xuw9T3PYyKlW1/FQlCpQ38N1U9bSFKvQ','سارة أحمد','student','university','active',NULL,1791395209523,NULL);
INSERT INTO "users" VALUES('e19c22a2-8340-4b82-a6f2-6b7d2bee761c','s2@x.com','$argon2id$v=19$m=19456,t=2,p=1$WUJxDD2GsYto5c1brlRVmA$eiPU42yuMnYiq8hFgk10NEYAaVOupM19h/YmdbI/BaQ','عمر علي','student','university','active',NULL,1791395210029,NULL);
INSERT INTO "users" VALUES('5712db87-a1f6-418b-9261-8b83ad0f9a4e','s3@x.com','$argon2id$v=19$m=19456,t=2,p=1$W7s/pzLSLmAu0iNWn225Ow$z0m8qGMnZD6EV7thRjU3BLHQCK0vRNZbOqXl35s7n+k','ليلى','student','school','active',NULL,1791395210525,NULL);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_org_units_inst ON org_units(institution_id, parent_id);
CREATE INDEX idx_subjects_inst ON subjects(institution_id, unit_id);
CREATE INDEX idx_ts_subject ON teacher_subjects(subject_id, status);
CREATE INDEX idx_enroll_subject ON subject_enrollments(subject_id);
CREATE INDEX idx_follow_teacher ON teacher_follows(teacher_id);
CREATE INDEX idx_posts_subject ON posts(subject_id, status);
CREATE INDEX idx_posts_teacher ON posts(teacher_id);
CREATE INDEX idx_courses_subject ON courses(subject_id, status);
CREATE INDEX idx_lessons_course ON lessons(course_id, position);
CREATE INDEX idx_live_subject ON live_sessions(subject_id, starts_at);
CREATE INDEX idx_reviews_target ON reviews(target_type, target_id);
CREATE INDEX idx_notif_user ON notifications(user_id, created_at);
CREATE INDEX idx_assess_subject ON assessments(subject_id, status);
CREATE INDEX idx_attempts_assess ON attempts(assessment_id, student_id);
CREATE INDEX idx_attempts_student ON attempts(student_id, started_at);
CREATE INDEX idx_reports_target ON reports(target_type, target_id, status);
CREATE UNIQUE INDEX idx_reports_open_unique ON reports(reporter_id, target_type, target_id) WHERE status = 'open';
DELETE FROM "sqlite_sequence";
INSERT INTO "sqlite_sequence" VALUES('audit_log',7);
COMMIT;
