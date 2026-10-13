//! Saved items (phase 4-2): a student bookmarks a post or a course to find it again (`PUT /bookmarks`,
//! `DELETE /bookmarks/{type}/{id}`, `GET /my/bookmarks`).
//!
//! A bookmark only ever points at something students can see **right now** (`platform_content::visible`, the one visibility
//! rule): a target that is hidden, unpublished or deleted cannot be saved (404) and is neither listed nor counted. A saved
//! item that is hidden later keeps its row (it comes back when the content does) but disappears from the list, and removing
//! a bookmark always works. Everything is scoped by the authenticated student's id; deleting a post or a course deletes its
//! bookmarks (`platform_ops::purge_target_references`).

use crate::platform::{bad, db_err, lock, require_role, Res, User};
use crate::platform_content::{visible, visible_to_public};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, put},
    Json, Router,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS bookmarks (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_type TEXT NOT NULL CHECK (target_type IN ('post','course')),
  target_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, target_type, target_id)
);
-- deleting a post or a course removes the bookmarks that point at it
CREATE INDEX IF NOT EXISTS idx_bookmarks_target ON bookmarks(target_type, target_id);
";

/// At most this many saved items that are shown right now per student (409 `too_many_bookmarks` beyond).
pub const MAX_BOOKMARKS: i64 = 200;

#[derive(Deserialize, Debug)]
pub struct BookmarkReq {
    pub target_type: String,
    pub target_id: String,
}

/// A saved item as the list shows it.
#[derive(Serialize, Debug, PartialEq)]
pub struct BookmarkRow {
    pub target_type: String,
    pub target_id: String,
    pub title: String,
    pub subject_name: String,
    pub teacher_name: String,
    pub saved_at: i64,
    pub updated_at: i64,
}

/// The student's saved items that students can see right now, newest saved first (ties by kind and id), optionally cut at
/// `limit`. **The only place that decides what counts as a saved item** — the list and the cap both read it.
fn shown_bookmarks(conn: &Connection, student_id: &str, limit: Option<i64>) -> Res<Vec<BookmarkRow>> {
    let sql = format!(
        "SELECT * FROM (
           SELECT 'post' AS t, p.id AS id, p.title AS title, s.name_ar AS subject, u.full_name AS teacher, b.created_at AS saved, p.updated_at AS updated
             FROM bookmarks b JOIN posts p ON b.target_type = 'post' AND p.id = b.target_id
             JOIN users u ON u.id = p.teacher_id JOIN subjects s ON s.id = p.subject_id
            WHERE b.user_id = ?1 AND {}
           UNION ALL
           SELECT 'course', c.id, c.title, s.name_ar, u.full_name, b.created_at, c.updated_at
             FROM bookmarks b JOIN courses c ON b.target_type = 'course' AND c.id = b.target_id
             JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id
            WHERE b.user_id = ?1 AND {}
         ) ORDER BY saved DESC, t, id LIMIT ?2",
        visible("p", "published"),
        visible("c", "published")
    );
    conn.prepare(&sql)
        .map_err(db_err)?
        .query_map(params![student_id, limit.unwrap_or(i64::MAX)], |r| {
            Ok(BookmarkRow { target_type: r.get(0)?, target_id: r.get(1)?, title: r.get(2)?, subject_name: r.get(3)?, teacher_name: r.get(4)?, saved_at: r.get(5)?, updated_at: r.get(6)? })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

#[derive(Serialize, Debug)]
pub struct BookmarkList {
    pub items: Vec<BookmarkRow>,
}

pub(crate) fn my_bookmarks(conn: &Connection, student: &User) -> Res<BookmarkList> {
    Ok(BookmarkList { items: shown_bookmarks(conn, &student.id, Some(MAX_BOOKMARKS))? })
}

/// Saves a post or a course. Idempotent (saving again changes nothing, not even at the cap). 400 `invalid_target` for another
/// kind, 404 `not_found` when the target does not exist or is not shown to students, 409 `too_many_bookmarks` at the cap.
pub(crate) fn add_bookmark(conn: &Connection, student: &User, req: &BookmarkReq, now: i64) -> Res<()> {
    let (table, alias) = match req.target_type.as_str() {
        "post" => ("posts", "p"),
        "course" => ("courses", "c"),
        _ => return Err(bad("invalid_target")),
    };
    if !visible_to_public(conn, table, alias, &req.target_id, "published")? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let has: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM bookmarks WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3)",
            params![student.id, req.target_type, req.target_id],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if has {
        return Ok(());
    }
    if shown_bookmarks(conn, &student.id, None)?.len() as i64 >= MAX_BOOKMARKS {
        return Err(err(StatusCode::CONFLICT, "too_many_bookmarks"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO bookmarks(user_id, target_type, target_id, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![student.id, req.target_type, req.target_id, now],
    )
    .map_err(db_err)?;
    Ok(())
}

/// Removes a saved item; always works (also for one whose target is hidden now or gone) and is idempotent. Only the caller's own row.
pub(crate) fn remove_bookmark(conn: &Connection, student: &User, target_type: &str, target_id: &str) -> Res<()> {
    conn.execute("DELETE FROM bookmarks WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3", params![student.id, target_type, target_id]).map_err(db_err)?;
    Ok(())
}

pub async fn put_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(req): Json<BookmarkReq>) -> Res<StatusCode> {
    let student = require_role(&s, &h, "student")?;
    add_bookmark(&*lock(&s)?, &student, &req, now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path((target_type, target_id)): Path<(String, String)>) -> Res<StatusCode> {
    let student = require_role(&s, &h, "student")?;
    remove_bookmark(&*lock(&s)?, &student, &target_type, &target_id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<BookmarkList>> {
    let student = require_role(&s, &h, "student")?;
    my_bookmarks(&*lock(&s)?, &student).map(Json)
}

/// The saved-items routes (merged into the app in `main.rs`).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/platform/bookmarks", put(put_handler))
        .route("/api/platform/bookmarks/{target_type}/{target_id}", delete(delete_handler))
        .route("/api/platform/my/bookmarks", get(list_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use axum::http::StatusCode;

    struct W {
        conn: Connection,
        teacher: User,
        me: User,
        other: User,
    }

    fn world() -> W {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0);",
        )
        .unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        conn.execute("UPDATE users SET full_name = 'المعلم' WHERE id = ?1", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at,decided_at) VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        let me = insert_test_user(&conn, "me@x.com", "student", "active");
        let other = insert_test_user(&conn, "o@x.com", "student", "active");
        W { conn, teacher, me, other }
    }

    fn post(w: &W, id: &str, status: &str, updated: i64) {
        w.conn
            .execute("INSERT INTO posts(id,teacher_id,subject_id,kind,title,body,status,created_at,updated_at) VALUES (?1,?2,'s1','article',?3,'نص',?4,0,?5)", params![id, w.teacher.id, format!("مقال {id}"), status, updated])
            .unwrap();
    }

    fn course(w: &W, id: &str, status: &str, updated: i64) {
        w.conn
            .execute("INSERT INTO courses(id,teacher_id,subject_id,title,status,created_at,updated_at) VALUES (?1,?2,'s1',?3,?4,0,?5)", params![id, w.teacher.id, format!("دورة {id}"), status, updated])
            .unwrap();
    }

    fn req(t: &str, id: &str) -> BookmarkReq {
        BookmarkReq { target_type: t.into(), target_id: id.into() }
    }

    fn save(w: &W, who: &User, t: &str, id: &str, at: i64) -> Res<()> {
        add_bookmark(&w.conn, who, &req(t, id), at)
    }

    fn listed(w: &W, who: &User) -> Vec<(String, String)> {
        my_bookmarks(&w.conn, who).unwrap().items.into_iter().map(|b| (b.target_type, b.target_id)).collect()
    }

    fn stored(w: &W) -> i64 {
        w.conn.query_row("SELECT count(*) FROM bookmarks", [], |r| r.get(0)).unwrap()
    }

    fn status<T>(r: Res<T>) -> StatusCode {
        r.err().expect("an error").0
    }

    #[test]
    fn a_post_and_a_course_are_saved_and_listed_newest_saved_first_with_their_details() {
        let w = world();
        post(&w, "p1", "published", 10);
        course(&w, "c1", "published", 20);
        save(&w, &w.me, "post", "p1", 100).unwrap();
        save(&w, &w.me, "course", "c1", 200).unwrap();
        assert_eq!(listed(&w, &w.me), [("course".to_string(), "c1".to_string()), ("post".to_string(), "p1".to_string())]);
        let rows = my_bookmarks(&w.conn, &w.me).unwrap().items;
        assert_eq!(
            rows[1],
            BookmarkRow { target_type: "post".into(), target_id: "p1".into(), title: "مقال p1".into(), subject_name: "برمجة".into(), teacher_name: "المعلم".into(), saved_at: 100, updated_at: 10 }
        );
        assert_eq!((rows[0].title.as_str(), rows[0].saved_at, rows[0].updated_at), ("دورة c1", 200, 20));
        // saved at the same instant: a stable order (kind, then id)
        save(&w, &w.me, "post", "p2-missing", 1).unwrap_err();
        post(&w, "p0", "published", 1);
        post(&w, "p9", "published", 1);
        save(&w, &w.me, "post", "p9", 300).unwrap();
        save(&w, &w.me, "post", "p0", 300).unwrap();
        assert_eq!(listed(&w, &w.me)[..2], [("post".to_string(), "p0".to_string()), ("post".to_string(), "p9".to_string())]);
    }

    #[test]
    fn saving_and_removing_are_idempotent_and_only_touch_the_callers_own_rows() {
        let w = world();
        post(&w, "p1", "published", 10);
        save(&w, &w.me, "post", "p1", 100).unwrap();
        save(&w, &w.me, "post", "p1", 999).unwrap();
        assert_eq!(stored(&w), 1);
        assert_eq!(my_bookmarks(&w.conn, &w.me).unwrap().items[0].saved_at, 100, "saving again changes nothing, not even the time");
        // somebody else's list is their own
        assert!(listed(&w, &w.other).is_empty());
        save(&w, &w.other, "post", "p1", 150).unwrap();
        assert_eq!(stored(&w), 2);
        remove_bookmark(&w.conn, &w.me, "post", "p1").unwrap();
        remove_bookmark(&w.conn, &w.me, "post", "p1").unwrap();
        remove_bookmark(&w.conn, &w.me, "course", "never-saved").unwrap();
        remove_bookmark(&w.conn, &w.me, "bogus", "x").unwrap();
        assert!(listed(&w, &w.me).is_empty());
        assert_eq!(listed(&w, &w.other), [("post".to_string(), "p1".to_string())], "removing mine never removes the other student's");
    }

    #[test]
    fn only_a_post_or_a_course_can_be_saved_and_only_one_students_can_see() {
        let w = world();
        post(&w, "p1", "published", 1);
        post(&w, "p-draft", "draft", 1);
        course(&w, "c-draft", "draft", 1);
        for t in ["live", "exam", "teacher", "subject", "", "POST", "post "] {
            assert_eq!(status(save(&w, &w.me, t, "p1", 1)), StatusCode::BAD_REQUEST, "target_type {t:?}");
        }
        for (t, id) in [("post", "nope"), ("course", "nope"), ("post", "p-draft"), ("course", "c-draft"), ("course", "p1"), ("post", "")] {
            assert_eq!(status(save(&w, &w.me, t, id, 1)), StatusCode::NOT_FOUND, "{t} {id}");
        }
        assert_eq!(stored(&w), 0);
        // hidden by each part of the visibility rule
        for (off, on) in [
            ("UPDATE teacher_subjects SET status = 'rejected'", "UPDATE teacher_subjects SET status = 'approved'"),
            ("UPDATE subjects SET is_active = 0", "UPDATE subjects SET is_active = 1"),
            ("UPDATE institutions SET is_active = 0", "UPDATE institutions SET is_active = 1"),
            ("UPDATE users SET status = 'suspended' WHERE role = 'teacher'", "UPDATE users SET status = 'active' WHERE role = 'teacher'"),
        ] {
            w.conn.execute(off, []).unwrap();
            assert_eq!(status(save(&w, &w.me, "post", "p1", 1)), StatusCode::NOT_FOUND, "{off}");
            w.conn.execute(on, []).unwrap();
        }
        assert_eq!(stored(&w), 0, "nothing was stored by a refused request");
        save(&w, &w.me, "post", "p1", 1).unwrap();
    }

    #[test]
    fn a_target_that_is_hidden_later_stays_stored_but_is_neither_listed_nor_counted() {
        let w = world();
        post(&w, "p1", "published", 1);
        course(&w, "c1", "published", 1);
        save(&w, &w.me, "post", "p1", 1).unwrap();
        save(&w, &w.me, "course", "c1", 2).unwrap();
        w.conn.execute("UPDATE posts SET status = 'draft'", []).unwrap();
        assert_eq!(listed(&w, &w.me), [("course".to_string(), "c1".to_string())]);
        assert_eq!(stored(&w), 2, "the row is kept: it comes back with the post");
        w.conn.execute("UPDATE teacher_subjects SET status = 'pending'", []).unwrap();
        assert!(listed(&w, &w.me).is_empty());
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved'", []).unwrap();
        w.conn.execute("UPDATE posts SET status = 'published'", []).unwrap();
        assert_eq!(listed(&w, &w.me).len(), 2);
        // removing a hidden one still works
        w.conn.execute("UPDATE posts SET status = 'draft'", []).unwrap();
        remove_bookmark(&w.conn, &w.me, "post", "p1").unwrap();
        assert_eq!(stored(&w), 1);
        // a deleted target leaves a row nobody can see (and never an error)
        w.conn.execute("DELETE FROM courses WHERE id = 'c1'", []).unwrap();
        assert!(listed(&w, &w.me).is_empty());
    }

    #[test]
    fn at_most_two_hundred_saved_items_that_are_shown() {
        let w = world();
        for n in 0..=MAX_BOOKMARKS {
            post(&w, &format!("p{n:03}"), "published", n);
        }
        for n in 0..MAX_BOOKMARKS {
            save(&w, &w.me, "post", &format!("p{n:03}"), n).unwrap();
        }
        assert_eq!(listed(&w, &w.me).len() as i64, MAX_BOOKMARKS);
        let extra = format!("p{:03}", MAX_BOOKMARKS);
        assert_eq!(status(save(&w, &w.me, "post", &extra, 999)), StatusCode::CONFLICT);
        let e = save(&w, &w.me, "post", &extra, 999).unwrap_err();
        assert!(e.1.contains("too_many_bookmarks"), "{}", e.1);
        // saving something already saved is not "one more"
        save(&w, &w.me, "post", "p005", 999).unwrap();
        // another student has their own 200
        save(&w, &w.other, "post", &extra, 1).unwrap();
        // room again after removing one
        remove_bookmark(&w.conn, &w.me, "post", "p000").unwrap();
        save(&w, &w.me, "post", &extra, 1000).unwrap();
        assert_eq!(listed(&w, &w.me).len() as i64, MAX_BOOKMARKS);
        // hidden rows do not use up the quota
        w.conn.execute("UPDATE posts SET status = 'draft' WHERE id IN ('p001','p002','p003')", []).unwrap();
        assert_eq!(listed(&w, &w.me).len() as i64, MAX_BOOKMARKS - 3);
        post(&w, "fresh", "published", 1);
        save(&w, &w.me, "post", "fresh", 1001).unwrap();
    }

    #[test]
    fn deleting_the_content_deletes_its_bookmarks_and_only_its_own() {
        let w = world();
        post(&w, "p1", "published", 1);
        post(&w, "p2", "published", 1);
        course(&w, "c1", "published", 1);
        for (t, id) in [("post", "p1"), ("post", "p2"), ("course", "c1")] {
            save(&w, &w.me, t, id, 1).unwrap();
            save(&w, &w.other, t, id, 1).unwrap();
        }
        assert_eq!(stored(&w), 6);
        crate::platform_ops::purge_target_references(&w.conn, "post", "p1").unwrap();
        assert_eq!(stored(&w), 4);
        // a course and a post with the same id are different targets
        post(&w, "same", "published", 1);
        course(&w, "same", "published", 1);
        save(&w, &w.me, "post", "same", 1).unwrap();
        save(&w, &w.me, "course", "same", 1).unwrap();
        crate::platform_ops::purge_target_references(&w.conn, "course", "same").unwrap();
        assert_eq!(listed(&w, &w.me).iter().filter(|(_, id)| id == "same").count(), 1);
        assert!(listed(&w, &w.me).contains(&("post".to_string(), "same".to_string())));
        // a teacher who deletes their account takes the bookmarks of everything they published with them
        crate::platform_ops::delete_account_and_files(&w.conn, std::path::Path::new("/nonexistent"), &w.teacher).unwrap();
        assert_eq!(stored(&w), 0);
    }

    #[test]
    fn a_students_own_account_deletion_takes_their_bookmarks_and_nobody_elses() {
        let w = world();
        post(&w, "p1", "published", 1);
        save(&w, &w.me, "post", "p1", 1).unwrap();
        save(&w, &w.other, "post", "p1", 1).unwrap();
        crate::platform_ops::delete_account_and_files(&w.conn, std::path::Path::new("/nonexistent"), &w.me).unwrap();
        assert_eq!(stored(&w), 1);
        assert_eq!(listed(&w, &w.other).len(), 1);
    }

    #[test]
    fn the_schema_is_idempotent_and_refuses_other_kinds() {
        let w = world();
        w.conn.execute_batch(SCHEMA).unwrap();
        w.conn.execute_batch(SCHEMA).unwrap();
        assert!(w.conn.execute("INSERT INTO bookmarks VALUES (?1,'live','x',1)", params![w.me.id]).is_err(), "the CHECK admits only posts and courses");
        assert!(w.conn.execute("INSERT INTO bookmarks VALUES ('ghost','post','x',1)", []).is_err(), "and a real user");
    }
}
