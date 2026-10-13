//! Phase 4-3, the hard end: `attempt_end` (one rule for when an attempt ends), the grace after it, settling at the closing
//! time, closing by hand, and the embargo of `after_close` results until the last attempt could have landed.

use super::fixtures::*;
use super::*;
use crate::platform_exam_admin::{act, update_exam, ExamReq};
use serde_json::json;

fn req(v: serde_json::Value) -> ExamReq {
    serde_json::from_value(v).unwrap()
}

// ───────── the one rule ─────────

#[test]
fn an_attempt_ends_at_the_earliest_of_its_duration_the_closing_time_and_a_manual_close() {
    let s = NOW;
    // duration only / duration shorter than the window / longer / equal
    assert_eq!(attempt_end(s, Some(30), None, None), s + 30 * MIN);
    assert_eq!(attempt_end(s, Some(30), Some(s + 90 * MIN), None), s + 30 * MIN, "the duration comes first");
    assert_eq!(attempt_end(s, Some(30), Some(s + 10 * MIN), None), s + 10 * MIN, "the closing time comes first");
    assert_eq!(attempt_end(s, Some(30), Some(s + 30 * MIN), None), s + 30 * MIN, "both at once");
    // an exam without a duration is open-ended for 24 hours at most
    assert_eq!(attempt_end(s, None, None, None), s + 24 * H);
    assert_eq!(attempt_end(s, None, Some(s + 5 * H), None), s + 5 * H);
    assert_eq!(attempt_end(s, None, Some(s + 48 * H), None), s + 24 * H, "a closing time beyond the 24 hours does not extend them");
    // closed by hand
    assert_eq!(attempt_end(s, Some(30), Some(s + 90 * MIN), Some(s + 5 * MIN)), s + 5 * MIN);
    assert_eq!(attempt_end(s, Some(30), None, Some(s + 50 * MIN)), s + 30 * MIN, "closed after the duration ran out: the duration");
    assert_eq!(attempt_end(s, None, None, Some(s + MIN)), s + MIN);
    // which of the two says "capped by closing"
    assert!(limited_by_closing(s, Some(30), Some(s + 10 * MIN), None));
    assert!(!limited_by_closing(s, Some(30), Some(s + 30 * MIN), None), "a tie is the duration's doing");
    assert!(!limited_by_closing(s, Some(30), Some(s + 60 * MIN), None));
    assert!(limited_by_closing(s, None, Some(s + 3 * H), None));
    assert!(!limited_by_closing(s, None, None, None) && !limited_by_closing(s, None, Some(s + 48 * H), None));
    assert!(limited_by_closing(s, Some(30), None, Some(s + MIN)), "a manual close counts as closing");
    // a manual close counts only while the exam IS closed
    assert_eq!(manual_close("closed", Some(7)), Some(7));
    assert_eq!(manual_close("archived", Some(7)), Some(7));
    assert_eq!(manual_close("published", Some(7)), None, "a stale value on a live exam ends nobody's attempt");
    assert_eq!(manual_close("draft", Some(7)), None);
    assert_eq!(manual_close("closed", None), None);
}

// ───────── starting and resuming ─────────

#[test]
fn a_start_near_the_closing_time_is_capped_by_it_and_says_so() {
    let w = world();
    // a 60-minute exam with 10 minutes of window left: the closing time wins
    let id = exam(&w, json!({"duration_min": 60, "closes_at": NOW + 10 * MIN}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW + MIN).unwrap();
    assert_eq!((s.ends_at, s.limited_by_closing, s.server_now, s.resumed), (NOW + 10 * MIN, true, NOW + MIN, false));
    let again = start_attempt(&w.conn, &w.student, &id, NOW + 2 * MIN).unwrap();
    assert_eq!((again.resumed, again.ends_at, again.limited_by_closing, again.server_now), (true, NOW + 10 * MIN, true, NOW + 2 * MIN), "resuming reports the same end");
    // a short exam inside a long window: the duration wins
    let short = exam(&w, json!({"title": "قصير", "duration_min": 5, "closes_at": NOW + 10 * MIN}));
    let s2 = start_attempt(&w.conn, &w.student, &short, NOW + MIN).unwrap();
    assert_eq!((s2.ends_at, s2.limited_by_closing), (NOW + 6 * MIN, false));
    // the duration ends exactly at the closing time: not "limited"
    let tie = exam(&w, json!({"title": "تعادل", "duration_min": 10, "closes_at": NOW + 10 * MIN}));
    let s3 = start_attempt(&w.conn, &w.student, &tie, NOW).unwrap();
    assert_eq!((s3.ends_at, s3.limited_by_closing), (NOW + 10 * MIN, false));
    // no duration: the closing time, or 24 hours when there is none either
    let open_ended = exam(&w, json!({"title": "مفتوح", "closes_at": NOW + 3 * H}));
    let s4 = start_attempt(&w.conn, &w.student, &open_ended, NOW).unwrap();
    assert_eq!((s4.ends_at, s4.limited_by_closing), (NOW + 3 * H, true));
    let unbounded = exam(&w, json!({"title": "بلا حدود"}));
    let s5 = start_attempt(&w.conn, &w.student, &unbounded, NOW).unwrap();
    assert_eq!((s5.ends_at, s5.limited_by_closing), (NOW + 24 * H, false));
}

#[test]
fn nobody_can_begin_an_attempt_at_or_after_the_closing_time() {
    let w = world();
    let id = exam(&w, json!({"closes_at": NOW + 10 * MIN, "max_attempts": 5}));
    let late = enroll(&w, "late@x.com");
    assert_eq!(fail(start_attempt(&w.conn, &late, &id, NOW + 10 * MIN)), (403, "closed".into()), "the very millisecond of the closing time: closed, like the exam page says");
    let last = enroll(&w, "last@x.com");
    let s = start_attempt(&w.conn, &last, &id, NOW + 10 * MIN - 1).unwrap();
    assert_eq!(s.ends_at, NOW + 10 * MIN, "a millisecond before: allowed, and over almost at once");
}

// ───────── the grace after the end ─────────

#[test]
fn a_running_attempt_can_be_saved_and_submitted_until_its_end_plus_grace_and_not_a_millisecond_longer() {
    let w = world();
    let c = NOW + 10 * MIN;
    let id = exam(&w, json!({"duration_min": 60, "closes_at": c, "max_attempts": 5}));
    let student = |n: &str| enroll(&w, &format!("{n}.student@x.com"));
    let (a, b, d, e) = (student("a"), student("b"), student("d"), student("e"));
    let start = |who: &User| start_attempt(&w.conn, who, &id, NOW).unwrap().attempt_id;
    let (ta, tb, td, te) = (start(&a), start(&b), start(&d), start(&e));
    for (who, t) in [(&a, &ta), (&b, &tb), (&d, &td), (&e, &te)] {
        save_answers(&w.conn, who, t, &ans(&[("q1", "B")]), NOW + MIN).unwrap();
    }
    // saving: accepted on the last millisecond of the grace, refused (and settled) one later
    assert!(save_answers(&w.conn, &a, &ta, &ans(&[("q1", "B"), ("q2", "AC")]), c + GRACE_MS).is_ok(), "end + grace: still running");
    assert_eq!(fail(save_answers(&w.conn, &b, &tb, &ans(&[("q1", "A")]), c + GRACE_MS + 1)), (409, "time_expired".into()), "one millisecond later");
    assert_eq!((status_of(&w, &tb).as_str(), submitted_at(&w, &tb), score_of(&w, &tb)), ("submitted", Some(c), 1.0), "settled from the earlier autosave, stamped at the END");
    // submitting: the same edges
    assert!(submit_attempt(&w.conn, &d, &td, &ans(&[("q1", "B"), ("q4", "مصر")]), c + GRACE_MS).is_ok(), "a submit on the last millisecond of the grace lands");
    assert_eq!(score_of(&w, &td), 2.0);
    assert_eq!(fail(submit_attempt(&w.conn, &e, &te, &ans(&[("q1", "B"), ("q4", "مصر")]), c + GRACE_MS + 1)), (409, "time_expired".into()));
    assert_eq!((status_of(&w, &te).as_str(), submitted_at(&w, &te), score_of(&w, &te)), ("submitted", Some(c), 1.0), "only what was autosaved counts");
    // nothing can be saved or submitted into a settled attempt
    assert_eq!(fail(save_answers(&w.conn, &b, &tb, &ans(&[]), c + 2 * GRACE_MS)).1, "already_submitted");
}

#[test]
fn settling_at_the_closing_time_submits_the_autosave_stamped_at_the_end_and_expires_a_silent_attempt() {
    let w = world();
    let c = NOW + 10 * MIN;
    let id = exam(&w, json!({"duration_min": 60, "closes_at": c}));
    let silent = enroll(&w, "silent@x.com");
    let saved = start_attempt(&w.conn, &w.student, &id, NOW).unwrap().attempt_id;
    let nothing = start_attempt(&w.conn, &silent, &id, NOW).unwrap().attempt_id;
    save_answers(&w.conn, &w.student, &saved, &ans(&[("q1", "B"), ("q2", "AC")]), NOW + MIN).unwrap();
    assert_eq!(settle_all(&w.conn, c).unwrap(), 0, "the closing time itself: the last saves may still be on their way");
    assert_eq!(settle_all(&w.conn, c + GRACE_MS).unwrap(), 0, "end + grace exactly");
    // the page the student lands on after the timer ran out reads the outcome instead of "in progress"
    let r = attempt_result_at(&w.conn, &w.student, &saved, c + GRACE_MS + 1).unwrap();
    assert_eq!((r.status.as_str(), r.score, r.submitted_at), ("submitted", 3.0, Some(c)));
    assert_eq!(settle_all(&w.conn, c + GRACE_MS + 1).unwrap(), 1, "the other one, nothing saved");
    assert_eq!((status_of(&w, &nothing).as_str(), submitted_at(&w, &nothing), score_of(&w, &nothing)), ("expired", None, 0.0));
    assert_eq!(settle_all(&w.conn, c + 5 * H).unwrap(), 0, "idempotent");
}

#[test]
fn extending_the_closing_time_lengthens_the_attempts_that_are_still_running() {
    let w = world();
    let id = exam(&w, json!({"closes_at": NOW + 10 * MIN}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    assert_eq!(s.ends_at, NOW + 10 * MIN);
    // extending after attempts exist is allowed (the closing time can only grow)
    update_exam(&w.conn, &w.admin, &id, &req(json!({"closes_at": NOW + 30 * MIN})), NOW + 5 * MIN).unwrap();
    let r = start_attempt(&w.conn, &w.student, &id, NOW + 6 * MIN).unwrap();
    assert_eq!((r.resumed, r.ends_at), (true, NOW + 30 * MIN), "derived on every read: the running attempt has more time");
    let row = &crate::platform_student::student_exams(&w.conn, &w.student.id, None, None, NOW + 6 * MIN).unwrap()[0];
    assert_eq!(row.in_progress_attempt.as_ref().map(|a| a.ends_at), Some(NOW + 30 * MIN), "the lists agree");
    // the old end + grace has gone by and the attempt still takes saves
    assert!(save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B")]), NOW + 15 * MIN).is_ok());
    assert_eq!(settle_all(&w.conn, NOW + 15 * MIN).unwrap(), 0, "not overdue any more");
    // …and it is settled at the NEW end
    assert_eq!(settle_all(&w.conn, NOW + 30 * MIN + GRACE_MS + 1).unwrap(), 1);
    assert_eq!(submitted_at(&w, &s.attempt_id), Some(NOW + 30 * MIN));
}

// ───────── closing by hand ─────────

#[test]
fn closing_by_hand_ends_every_running_attempt_at_once_and_reopening_does_not_bring_them_back() {
    let w = world();
    let a = exam(&w, json!({"closes_at": NOW + 5 * H, "max_attempts": 2}));
    let other = exam(&w, json!({"title": "آخر", "closes_at": NOW + 5 * H}));
    let (s2, s3, s4) = (enroll(&w, "s2@x.com"), enroll(&w, "s3@x.com"), enroll(&w, "s4@x.com"));
    // s1 autosaved two answers; s2 saved nothing; s3 handed in earlier; s4 is running on ANOTHER exam
    let a1 = start_attempt(&w.conn, &w.student, &a, NOW).unwrap().attempt_id;
    save_answers(&w.conn, &w.student, &a1, &ans(&[("q1", "B"), ("q2", "AC")]), NOW + MIN).unwrap();
    let a2 = start_attempt(&w.conn, &s2, &a, NOW).unwrap().attempt_id;
    let a3 = take(&w, &s3, &a, NOW, MIN, &all_right());
    let a4 = start_attempt(&w.conn, &s4, &other, NOW).unwrap().attempt_id;
    let before3 = (score_of(&w, &a3), submitted_at(&w, &a3));

    let closed = act(&w.conn, &w.admin, &a, "close", NOW + 10 * MIN).unwrap();
    assert_eq!(closed.row.info.status, "closed");
    assert_eq!((status_of(&w, &a1).as_str(), submitted_at(&w, &a1), score_of(&w, &a1)), ("submitted", Some(NOW + 10 * MIN), 3.0), "each student's autosave is graded, stamped at the close");
    assert_eq!((status_of(&w, &a2).as_str(), submitted_at(&w, &a2)), ("expired", None), "nothing saved: expired");
    assert_eq!((score_of(&w, &a3), submitted_at(&w, &a3)), before3, "a finished attempt is not touched");
    assert_eq!(status_of(&w, &a4), "in_progress", "another exam's attempts are not touched");
    assert_eq!(count(&w, &format!("SELECT count(*) FROM attempts WHERE assessment_id = '{a}' AND status = 'in_progress'")), 0);
    // nothing more can be written
    assert_eq!(fail(save_answers(&w.conn, &w.student, &a1, &ans(&[("q1", "A")]), NOW + 11 * MIN)).1, "already_submitted");
    assert_eq!(fail(submit_attempt(&w.conn, &w.student, &a1, &ans(&[("q1", "A")]), NOW + 11 * MIN)).1, "already_submitted");
    assert_eq!(fail(save_answers(&w.conn, &s2, &a2, &ans(&[]), NOW + 11 * MIN)).1, "time_expired");
    // a second close is refused (nothing to do), and settling again finds nothing
    assert_eq!(fail(act(&w.conn, &w.admin, &a, "close", NOW + 12 * MIN).map(|_| ())), (409, "invalid_transition".into()));
    assert_eq!(settle_on_close(&w.conn, &a, NOW + 12 * MIN).unwrap(), 0);
    assert_eq!(score_of(&w, &a1), 3.0);
    // reopening does not resurrect anything: the settled attempts stay settled, and a new one may start (one attempt left)
    act(&w.conn, &w.admin, &a, "reopen", NOW + 20 * MIN).unwrap();
    assert_eq!(count(&w, &format!("SELECT count(*) FROM attempts WHERE assessment_id = '{a}' AND status = 'in_progress'")), 0);
    assert_eq!(status_of(&w, &a1), "submitted");
    let fresh = start_attempt(&w.conn, &w.student, &a, NOW + 21 * MIN).unwrap();
    assert!(!fresh.resumed && fresh.attempt_id != a1, "a new attempt, not the old one");
}

#[test]
fn the_close_and_the_settlement_are_one_transaction() {
    let w = world();
    let id = exam(&w, json!({"closes_at": NOW + 5 * H}));
    let at = start_attempt(&w.conn, &w.student, &id, NOW).unwrap().attempt_id;
    save_answers(&w.conn, &w.student, &at, &ans(&[("q1", "B")]), NOW + MIN).unwrap();
    // a settlement that cannot be written
    w.conn.execute_batch("CREATE TRIGGER boom BEFORE UPDATE ON attempts BEGIN SELECT RAISE(ABORT, 'boom'); END;").unwrap();
    assert_eq!(act(&w.conn, &w.admin, &id, "close", NOW + 10 * MIN).unwrap_err().0, StatusCode::INTERNAL_SERVER_ERROR);
    let (status, closed_at): (String, Option<i64>) = w.conn.query_row("SELECT status, closed_at FROM assessments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((status.as_str(), closed_at), ("published", None), "the exam is not closed when its attempts could not be ended");
    assert_eq!(status_of(&w, &at), "in_progress");
    w.conn.execute_batch("DROP TRIGGER boom").unwrap();
    assert!(act(&w.conn, &w.admin, &id, "close", NOW + 10 * MIN).is_ok());
    assert_eq!(status_of(&w, &at), "submitted");
}

#[test]
fn a_teacher_closing_their_own_exam_ends_the_attempts_too() {
    let w = world();
    let id = exam_by(&w, &w.teacher, json!({"closes_at": NOW + 5 * H}));
    let at = start_attempt(&w.conn, &w.student, &id, NOW).unwrap().attempt_id;
    save_answers(&w.conn, &w.student, &at, &ans(&[("q1", "B"), ("q3", "A")]), NOW + MIN).unwrap();
    crate::platform_exam_teacher::lifecycle_checked(&w.conn, &crate::platform_settings::Crypto::for_tests(), &w.teacher, &id, "close", NOW + 3 * MIN).unwrap();
    assert_eq!((status_of(&w, &at).as_str(), submitted_at(&w, &at), score_of(&w, &at)), ("submitted", Some(NOW + 3 * MIN), 2.0));
}

// ───────── the embargo ─────────

#[test]
fn the_embargo_rule_is_decided_in_one_function() {
    let c = 5_000_000;
    assert!(is_released("immediate", "published", Some(c), 0) && is_released("immediate", "published", None, 0), "immediate exams are never withheld");
    assert!(!is_released("after_close", "published", None, c * 100), "no closing time: nothing to wait for, nothing released by time");
    assert!(!is_released("after_close", "published", Some(c), c - 1));
    assert!(!is_released("after_close", "published", Some(c), c), "the closing time itself");
    assert!(!is_released("after_close", "published", Some(c), c + GRACE_MS), "the last millisecond an attempt can still land");
    assert!(is_released("after_close", "published", Some(c), c + GRACE_MS + 1), "the first millisecond nothing can land any more");
    // closed by hand: everything was ended at the close, so there is nothing left to wait for
    assert!(is_released("after_close", "closed", Some(c * 10), 0));
    assert!(is_released("after_close", "closed", None, 0));
    assert!(is_released("after_close", "archived", Some(c * 10), 0));
}

#[test]
fn after_close_results_and_answers_stay_withheld_until_the_last_attempt_could_have_landed() {
    let w = world();
    let c = NOW + H;
    let id = exam(&w, json!({"release_mode": "after_close", "closes_at": c, "pass_mark": 50}));
    // handed in long before the closing time, and fully graded (the written answer was left blank)
    let at = take(&w, &w.student, &id, NOW, MIN, &objective_right());
    for (now, released) in [(NOW + 2 * MIN, false), (c - 1, false), (c, false), (c + GRACE_MS, false), (c + GRACE_MS + 1, true), (c + 10 * H, true)] {
        let r = attempt_result_at(&w.conn, &w.student, &at, now).unwrap();
        let mine = &my_attempts_at(&w.conn, &w.student.id, None, now).unwrap()[0];
        assert_eq!((r.released, mine.released), (released, released), "released at {now}");
        if released {
            assert_eq!((r.score, r.items.is_some(), r.passed, r.release_at), (5.0, true, Some(true), None), "at {now}");
            assert_eq!((mine.score, mine.passed), (5.0, Some(true)));
        } else {
            assert_eq!((r.score, r.pending, r.passed, r.release_at), (0.0, 0, None, Some(c)), "at {now}: nothing about the outcome");
            assert!(r.items.is_none(), "at {now}: no answers, no key");
            assert_eq!((mine.score, mine.passed), (0.0, None));
            let json = serde_json::to_string(&r).unwrap();
            assert!(!json.contains("correct_answer") && !json.contains("your_answer"), "{json}");
        }
        // graders always see everything
        let g = attempt_result_at(&w.conn, &w.admin, &at, now).unwrap();
        assert!(g.released && g.items.is_some() && g.score == 5.0, "at {now}");
    }
    // the student overview and best-attempt rows follow the same rule
    let row = |now: i64| crate::platform_student::student_exams(&w.conn, &w.student.id, None, None, now).unwrap().remove(0);
    assert!(!row(c + GRACE_MS).best.unwrap().released && row(c + GRACE_MS + 1).best.unwrap().released);
}

#[test]
fn closing_by_hand_releases_the_results_at_once_even_before_the_closing_time() {
    let w = world();
    let id = exam(&w, json!({"release_mode": "after_close", "closes_at": NOW + 10 * H}));
    let at = take(&w, &w.student, &id, NOW, MIN, &objective_right());
    assert!(!attempt_result_at(&w.conn, &w.student, &at, NOW + 5 * MIN).unwrap().released);
    act(&w.conn, &w.admin, &id, "close", NOW + 6 * MIN).unwrap();
    let r = attempt_result_at(&w.conn, &w.student, &at, NOW + 6 * MIN).unwrap();
    assert_eq!((r.released, r.score, r.items.is_some()), (true, 5.0, true), "everything was ended at the close: nothing left to wait for");
    // archived stays released
    act(&w.conn, &w.admin, &id, "archive", NOW + 7 * MIN).unwrap();
    assert!(attempt_result_at(&w.conn, &w.student, &at, NOW + 8 * MIN).unwrap().released);
}

#[test]
fn nobody_reads_the_key_while_a_classmate_can_still_write() {
    // the scenario the embargo exists for: A finishes early, B is still writing at the closing time
    let w = world();
    let c = NOW + H;
    let id = exam(&w, json!({"release_mode": "after_close", "closes_at": c, "duration_min": 120}));
    let b = enroll(&w, "b@x.com");
    let a_attempt = take(&w, &w.student, &id, NOW, 5 * MIN, &objective_right());
    let b_attempt = start_attempt(&w.conn, &b, &id, c - 5 * MIN).unwrap();
    save_answers(&w.conn, &b, &b_attempt.attempt_id, &ans(&[("q1", "B")]), c - MIN).unwrap();
    // B's attempt ends at the closing time (not 2 h later), and A cannot see anything until B's last save could land
    assert_eq!(b_attempt.ends_at, c);
    assert!(!attempt_result_at(&w.conn, &w.student, &a_attempt, c + GRACE_MS).unwrap().released);
    assert!(submit_attempt(&w.conn, &b, &b_attempt.attempt_id, &ans(&[("q1", "B"), ("q2", "AC")]), c + GRACE_MS).is_ok(), "B's last possible submission");
    assert!(attempt_result_at(&w.conn, &w.student, &a_attempt, c + GRACE_MS + 1).unwrap().released, "only now");
}

// ───────── settlement is lazy: moving the closing time must not undo an end that already happened ─────────

#[test]
fn extending_the_closing_time_does_not_revive_an_attempt_that_was_already_over() {
    let w = world();
    let c = NOW + 10 * MIN;
    let id = exam(&w, json!({"duration_min": 60, "closes_at": c, "max_attempts": 1}));
    let saved = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    save_answers(&w.conn, &w.student, &saved.attempt_id, &ans(&[("q1", "B")]), NOW + MIN).unwrap();
    let silent_student = enroll(&w, "silent@x.com");
    let silent = start_attempt(&w.conn, &silent_student, &id, NOW).unwrap();
    // nobody touched the first two after the closing time, so the table still says `in_progress` although they are long over
    let over_for_good = c + GRACE_MS + 1_000;
    assert_eq!((status_of(&w, &saved.attempt_id).as_str(), status_of(&w, &silent.attempt_id).as_str()), ("in_progress", "in_progress"));
    // the owner extends the closing time now: they were over, and stay over
    update_exam(&w.conn, &w.admin, &id, &req(json!({"closes_at": NOW + 3 * H})), over_for_good).unwrap();
    assert_eq!((status_of(&w, &saved.attempt_id).as_str(), submitted_at(&w, &saved.attempt_id), score_of(&w, &saved.attempt_id)), ("submitted", Some(c), 1.0), "settled from the autosave at their END");
    assert_eq!(status_of(&w, &silent.attempt_id), "expired");
    assert_eq!(fail(submit_attempt(&w.conn, &w.student, &saved.attempt_id, &ans(&[("q1", "B"), ("q2", "AC")]), over_for_good + 5 * MIN)).1, "already_submitted", "no coming back to write more");
    assert_eq!(fail(save_answers(&w.conn, &silent_student, &silent.attempt_id, &ans(&[("q1", "B")]), over_for_good + 5 * MIN)).1, "time_expired");
}

#[test]
fn an_extension_inside_the_grace_still_lengthens_the_attempt_and_the_save_reply_says_so() {
    let w = world();
    let c = NOW + 10 * MIN;
    let id = exam(&w, json!({"duration_min": 60, "closes_at": c}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B")]), c - 30_000).unwrap();
    // the closing time has been reached but the grace is not over: the attempt is still running and the owner moves the end
    let inside_grace = c + GRACE_MS - 1_000;
    update_exam(&w.conn, &w.admin, &id, &req(json!({"closes_at": NOW + 40 * MIN})), inside_grace).unwrap();
    assert_eq!(status_of(&w, &s.attempt_id), "in_progress", "not over: its end is derived from the new closing time");
    let r = start_attempt(&w.conn, &w.student, &id, inside_grace + MIN).unwrap();
    assert_eq!((r.resumed, r.ends_at, r.limited_by_closing), (true, NOW + 40 * MIN, true));
    let saved = save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q2", "AC")]), false, inside_grace + 2 * MIN).unwrap();
    assert_eq!((saved.ends_at, saved.limited_by_closing), (NOW + 40 * MIN, true), "the autosave reply carries the end a page that is already open must adopt");
    assert!(submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[]), inside_grace + 3 * MIN).is_ok());
    assert_eq!(score_of(&w, &s.attempt_id), 3.0, "the merge brought the autosaved answers into the final sheet");
}

#[test]
fn the_legacy_update_route_settles_before_it_moves_the_closing_time_too() {
    let w = world();
    let real = now_ms();
    let id = exam_by(&w, &w.teacher, json!({"duration_min": 60, "closes_at": real + 30 * MIN, "max_attempts": 1}));
    let s = start_attempt(&w.conn, &w.student, &id, real - 20 * MIN).unwrap();
    save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B")]), real - 19 * MIN).unwrap();
    // the closing time was reached two minutes ago, long enough for the grace to be over (the table was never touched since)
    w.conn.execute("UPDATE assessments SET closes_at = ?1 WHERE id = ?2", params![real - 5 * MIN, id]).unwrap();
    assert_eq!(status_of(&w, &s.attempt_id), "in_progress");
    let r: AssessmentReq = serde_json::from_value(json!({"closes_at": real + 3 * H})).unwrap();
    update_assessment(&w.conn, &w.teacher, &id, &r).unwrap();
    assert_eq!((status_of(&w, &s.attempt_id).as_str(), submitted_at(&w, &s.attempt_id)), ("submitted", Some(real - 5 * MIN)), "ended at the old closing time, not revived by the new one");
}

// ───────── settling many attempts at once ─────────

#[test]
fn a_hard_end_settles_a_whole_class_in_one_pass_and_one_bad_attempt_does_not_hold_the_others_back() {
    let w = world();
    let c = NOW + 10 * MIN;
    let id = exam(&w, json!({"duration_min": 60, "closes_at": c}));
    let ok_exam = id.clone();
    let broken = exam(&w, json!({"title": "معطوب", "duration_min": 60, "closes_at": c}));
    let mut ok = vec![];
    for i in 0..25 {
        let who = enroll(&w, &format!("c{i}@x.com"));
        let at = start_attempt(&w.conn, &who, &ok_exam, NOW).unwrap().attempt_id;
        save_answers(&w.conn, &who, &at, &ans(&[("q1", "B")]), NOW + MIN).unwrap();
        ok.push(at);
    }
    let bad_student = enroll(&w, "bad@x.com");
    let bad = start_attempt(&w.conn, &bad_student, &broken, NOW).unwrap().attempt_id;
    save_answers(&w.conn, &bad_student, &bad, &ans(&[("q1", "B")]), NOW + MIN).unwrap();
    // one exam's questions cannot be read any more: its attempt cannot be graded
    w.conn.execute("UPDATE assessments SET questions = 'not json' WHERE id = ?1", params![broken]).unwrap();
    let e = settle_all(&w.conn, c + GRACE_MS + 1).unwrap_err();
    assert_eq!(e.0, StatusCode::INTERNAL_SERVER_ERROR, "the failure is reported, not swallowed");
    assert!(ok.iter().all(|a| status_of(&w, a) == "submitted"), "…but the other 25 were settled");
    assert_eq!(status_of(&w, &bad), "in_progress", "the failing one was rolled back on its own, untouched");
    // repaired, the next sweep finishes it
    w.conn.execute("UPDATE assessments SET questions = ?1 WHERE id = ?2", params![questions().to_string(), broken]).unwrap();
    assert_eq!(settle_all(&w.conn, c + GRACE_MS + 2).unwrap(), 1);
    assert_eq!(status_of(&w, &bad), "submitted");
}

// ───────── a sitting survives its exam being hidden ─────────

#[test]
fn an_enrolled_student_gets_back_into_a_running_attempt_after_the_exam_was_hidden_but_cannot_start_another() {
    let w = world();
    let id = exam(&w, json!({"max_attempts": 3}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B")]), NOW + MIN).unwrap();
    // the subject is switched off while they sit it
    w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
    let r = start_attempt(&w.conn, &w.student, &id, NOW + 2 * MIN).unwrap();
    assert!(r.resumed && r.attempt_id == s.attempt_id, "back into the same attempt");
    assert_eq!(r.saved_answers, ans(&[("q1", "B")]));
    assert_eq!(r.ends_at, s.ends_at);
    assert!(save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q2", "AC")]), NOW + 3 * MIN).is_ok());
    assert!(submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[]), NOW + 4 * MIN).is_ok(), "and they can hand it in");
    assert_eq!(score_of(&w, &s.attempt_id), 3.0);
    // …but a hidden exam starts nothing new, for an enrolled student ...
    assert_eq!(fail(start_attempt(&w.conn, &w.student, &id, NOW + 5 * MIN)), (404, "not_found".into()));
    // ... and a student who is not in the subject learns nothing about it
    let outsider = crate::platform::insert_test_user(&w.conn, "out@x.com", "student", "active");
    assert_eq!(fail(start_attempt(&w.conn, &outsider, &id, NOW + 5 * MIN)), (404, "not_found".into()));
    // a running attempt does not get a student back in once they left the subject
    w.conn.execute("UPDATE subjects SET is_active = 1 WHERE id = 's1'", []).unwrap();
    let again = start_attempt(&w.conn, &w.student, &id, NOW + 6 * MIN).unwrap();
    assert!(!again.resumed);
    w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
    w.conn.execute("DELETE FROM subject_enrollments WHERE student_id = ?1", params![w.student.id]).unwrap();
    assert_eq!(fail(start_attempt(&w.conn, &w.student, &id, NOW + 7 * MIN)), (404, "not_found".into()), "not enrolled: nothing to resume");
    w.conn.execute("UPDATE subjects SET is_active = 1 WHERE id = 's1'", []).unwrap();
    assert_eq!(fail(start_attempt(&w.conn, &w.student, &id, NOW + 8 * MIN)), (403, "not_enrolled".into()), "visible and not enrolled: refused as ever");
}
