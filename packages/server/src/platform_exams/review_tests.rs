//! Phase 4-3: partial autosave, the review's extra fields (`your_answer_text`, chapter, difficulty, attempt number, best
//! badge, duration) and the fill-in-the-blank normalisation shared with the browser through
//! `packages/shared/fixtures/fill-blank-vectors.json`.

use super::fixtures::*;
use super::*;
use rand::{rngs::StdRng, SeedableRng};
use serde_json::{json, Value};

fn stored(w: &W, attempt: &str) -> HashMap<String, String> {
    let raw: String = w.conn.query_row("SELECT answers FROM attempts WHERE id = ?1", params![attempt], |r| r.get(0)).unwrap();
    serde_json::from_str(&raw).unwrap()
}

// ───────── partial autosave ─────────

#[test]
fn a_partial_save_merges_so_two_devices_saving_different_questions_never_erase_each_other() {
    let w = world();
    let id = exam(&w, json!({}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    // the phone saves q1, the laptop saves q2: both are kept
    let first = save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B")]), false, NOW + 1_000).unwrap();
    assert_eq!((first.saved_at, first.answered, first.ends_at, first.limited_by_closing), (NOW + 1_000, 1, s.ends_at, false));
    let second = save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q2", "AC")]), false, NOW + 2_000).unwrap();
    assert_eq!((second.saved_at, second.answered), (NOW + 2_000, 2));
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q1", "B"), ("q2", "AC")]), "the union of the two partial saves");
    // changing one question leaves the rest
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "C")]), false, NOW + 3_000).unwrap();
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q1", "C"), ("q2", "AC")]));
    // an empty string removes that answer, and only that one
    let cleared = save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "")]), false, NOW + 4_000).unwrap();
    assert_eq!(cleared.answered, 1);
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q2", "AC")]));
    // an empty request changes nothing (the time of the save is still recorded)
    let nothing = save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[]), false, NOW + 5_000).unwrap();
    assert_eq!((nothing.answered, stored(&w, &s.attempt_id).len(), nothing.saved_at), (1, 1, NOW + 5_000));
    // the resume on any device sees the merged sheet
    let r = start_attempt(&w.conn, &w.student, &id, NOW + 6_000).unwrap();
    assert!(r.resumed && r.saved_answers == ans(&[("q2", "AC")]) && r.saved_at == Some(NOW + 5_000));
}

#[test]
fn a_partial_save_keeps_only_this_attempts_questions_cuts_long_values_and_counts_real_answers() {
    let w = world();
    let id = exam(&w, json!({}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    let long = "ك".repeat(3_000);
    let saved = save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("ghost", "x"), ("q5", &long), ("q4", "   ")]), false, NOW + 1).unwrap();
    let kept = stored(&w, &s.attempt_id);
    assert!(!kept.contains_key("ghost"), "ids of other questions are dropped");
    assert_eq!(kept["q5"].chars().count(), 2_000, "cut at 2000 characters, like a submit");
    assert_eq!(saved.answered, 1, "a value of only spaces is not an answer (it is stored, but not counted)");
    // …and a ghost key cannot be used to wipe anything either
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("ghost", "")]), false, NOW + 2).unwrap();
    assert_eq!(stored(&w, &s.attempt_id).len(), kept.len());
}

#[test]
fn replace_keeps_the_older_whole_sheet_behaviour() {
    let w = world();
    let id = exam(&w, json!({}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B"), ("q2", "AC"), ("q3", "A")]), false, NOW + 1).unwrap();
    let r = save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q4", "مصر")]), true, NOW + 2).unwrap();
    assert_eq!(r.answered, 1);
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q4", "مصر")]), "the request IS the whole answer sheet");
    // exactly as before: an empty value is stored as given, and an empty sheet empties the attempt
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "")]), true, NOW + 3).unwrap();
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q1", "")]));
    assert_eq!(save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[]), true, NOW + 4).unwrap().answered, 0);
    assert!(stored(&w, &s.attempt_id).is_empty());
}

#[test]
fn partial_saves_keep_the_displayed_letter_convention_and_grade_by_option_text() {
    let w = world();
    let id = exam(&w, json!({"shuffle_options": true}));
    let s = start_attempt_with(&w.conn, &w.student, &id, NOW, &mut StdRng::seed_from_u64(11)).unwrap();
    let shown = |qid: &str| s.questions.iter().find(|q| q.id == qid).unwrap().options.clone();
    // the displayed letters whose text is the correct option(s)
    let letter = |qid: &str, text: &str| (b'A' + shown(qid).iter().position(|t| t == text).unwrap() as u8) as char;
    let q1 = letter("q1", "o1-1").to_string();
    let q2: String = ["o2-0", "o2-2"].iter().map(|t| letter("q2", t)).collect();
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", &q1)]), false, NOW + 1).unwrap();
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q2", &q2)]), false, NOW + 2).unwrap();
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q1", &q1), ("q2", &q2)]), "stored as displayed");
    let r = settle_all(&w.conn, NOW + 25 * H).unwrap();
    assert_eq!(r, 1);
    assert_eq!(score_of(&w, &s.attempt_id), 3.0, "both are right once mapped back to the original options");
    assert_eq!(stored(&w, &s.attempt_id)["q2"], "AC", "a settled attempt stores original letters");
}

#[test]
fn saving_errors_are_the_same_as_before() {
    let w = world();
    let id = exam(&w, json!({"duration_min": 10}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    let other = enroll(&w, "o@x.com");
    assert_eq!(fail(save_answers_with(&w.conn, &other, &s.attempt_id, &ans(&[]), false, NOW + 1)), (404, "not_found".into()), "another student's attempt");
    assert_eq!(fail(save_answers_with(&w.conn, &w.student, "nope", &ans(&[]), false, NOW + 1)), (404, "not_found".into()));
    assert_eq!(fail(save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B")]), false, NOW + 10 * MIN + GRACE_MS + 1)), (409, "time_expired".into()));
    // that late save settled it: nothing had been saved, so it is expired, and a further save says so
    assert_eq!(fail(save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[]), false, NOW + 11 * MIN)), (409, "time_expired".into()));
    let s2 = start_attempt(&w.conn, &w.student, &id, NOW + 20 * MIN).unwrap();
    submit_attempt(&w.conn, &w.student, &s2.attempt_id, &ans(&[("q1", "B")]), NOW + 21 * MIN).unwrap();
    assert_eq!(fail(save_answers_with(&w.conn, &w.student, &s2.attempt_id, &ans(&[("q1", "A")]), false, NOW + 22 * MIN)), (409, "already_submitted".into()));
}

// ───────── the final hand-in keeps what a second device saved ─────────

#[test]
fn the_hand_in_is_laid_over_the_autosave_so_a_stale_page_cannot_erase_another_devices_answers() {
    let w = world();
    let id = exam(&w, json!({}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    // the laptop loaded the attempt early; the phone then saved q3 and q4 (partial saves merge)
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B"), ("q2", "AC")]), false, NOW + 1_000).unwrap();
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q3", "A"), ("q4", "مصر")]), false, NOW + 2_000).unwrap();
    // the laptop, which only knows q1 and q2, hands in
    let r = submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B"), ("q2", "AC")]), NOW + 3_000).unwrap();
    assert_eq!(r.score, 5.0, "q3 and q4 were saved on the other device and are graded");
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q1", "B"), ("q2", "AC"), ("q3", "A"), ("q4", "مصر")]));
}

#[test]
fn what_the_hand_in_says_wins_and_an_empty_string_clears_an_autosaved_answer() {
    let w = world();
    let id = exam(&w, json!({}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "A"), ("q2", "AC"), ("q3", "B")]), false, NOW + 1_000).unwrap();
    // q1 changed on this page, q2 was cleared on this page, q3 is not mentioned (kept), q4 is new
    let r = submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B"), ("q2", ""), ("q4", "مصر")]), NOW + 2_000).unwrap();
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q1", "B"), ("q3", "B"), ("q4", "مصر")]));
    assert_eq!(r.score, 2.0, "q1 and q4 are right; q3 is kept but wrong; q2 was cleared");
    // ids of other questions are dropped and long values cut, as in a save
    let again = start_attempt(&w.conn, &w.student, &id, NOW + 3_000).unwrap();
    let long = "ك".repeat(3_000);
    submit_attempt(&w.conn, &w.student, &again.attempt_id, &ans(&[("ghost", "x"), ("q5", &long)]), NOW + 4_000).unwrap();
    let kept = stored(&w, &again.attempt_id);
    assert!(!kept.contains_key("ghost") && kept["q5"].chars().count() == 2_000);
}

#[test]
fn replace_on_the_hand_in_keeps_the_older_whole_sheet_behaviour() {
    let w = world();
    let id = exam(&w, json!({}));
    let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B"), ("q2", "AC")]), false, NOW + 1_000).unwrap();
    let r = submit_attempt_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q3", "A")]), true, NOW + 2_000).unwrap();
    assert_eq!(r.score, 1.0, "the request IS the whole sheet");
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q3", "A")]));
}

#[test]
fn the_hand_in_merge_works_in_displayed_letters_when_options_are_shuffled() {
    let w = world();
    let id = exam(&w, json!({"shuffle_options": true}));
    let s = start_attempt_with(&w.conn, &w.student, &id, NOW, &mut StdRng::seed_from_u64(5)).unwrap();
    let shown = |qid: &str| s.questions.iter().find(|q| q.id == qid).unwrap().options.clone();
    let letter = |qid: &str, text: &str| (b'A' + shown(qid).iter().position(|t| t == text).unwrap() as u8) as char;
    let q1 = letter("q1", "o1-1").to_string();
    let q2: String = ["o2-0", "o2-2"].iter().map(|t| letter("q2", t)).collect();
    save_answers_with(&w.conn, &w.student, &s.attempt_id, &ans(&[("q2", &q2)]), false, NOW + 1_000).unwrap();
    let r = submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", &q1)]), NOW + 2_000).unwrap();
    assert_eq!(r.score, 3.0, "the saved q2 and the submitted q1 are both mapped back to the original options");
    assert_eq!(stored(&w, &s.attempt_id), ans(&[("q1", "B"), ("q2", "AC")]));
}

// ───────── your_answer_text ─────────

fn question(v: Value) -> Question {
    serde_json::from_value(v).unwrap()
}

#[test]
fn the_answer_in_words_names_the_chosen_options_in_their_original_order() {
    let single = question(json!({"id":"a","type":"single_choice","stem":"س","options":["أول","ثان","ثالث"],"answer":"B"}));
    let multi = question(json!({"id":"b","type":"multi_choice","stem":"س","options":["أول","ثان","ثالث","رابع"],"answer":"AC"}));
    let tf = question(json!({"id":"c","type":"true_false","stem":"س","answer":"A"}));
    let tf_named = question(json!({"id":"d","type":"true_false","stem":"س","options":["True","False"],"answer":"A"}));
    let fill = question(json!({"id":"e","type":"fill_blank","stem":"س","answer":"x"}));
    let short = question(json!({"id":"f","type":"short_answer","stem":"س","answer":"x"}));
    assert_eq!(answer_text(&single, Some("B")).as_deref(), Some("ثان"));
    assert_eq!(answer_text(&multi, Some("CA")).as_deref(), Some("أول | ثالث"), "original option order, whatever order the letters are stored in");
    assert_eq!(answer_text(&multi, Some("ACAC")).as_deref(), Some("أول | ثالث"), "no repeats");
    assert_eq!(answer_text(&multi, Some("a, c")).as_deref(), Some("أول | ثالث"), "lower case and separators are read like the marking does");
    assert_eq!(answer_text(&single, Some("Z")), None, "no option has that letter: the browser falls back to the letter");
    assert_eq!(answer_text(&single, Some("E")), None, "a letter past the last option");
    // true / false: the option text, or the default words when the question names none
    assert_eq!((answer_text(&tf, Some("A")).as_deref(), answer_text(&tf, Some("B")).as_deref()), (Some("صحيح"), Some("خطأ")));
    assert_eq!(answer_text(&tf, Some("صح")).as_deref(), Some("صحيح"), "Arabic words are read like the marking does");
    assert_eq!(answer_text(&tf, Some("خطأ")).as_deref(), Some("خطأ"));
    assert_eq!((answer_text(&tf_named, Some("A")).as_deref(), answer_text(&tf_named, Some("B")).as_deref()), (Some("True"), Some("False")));
    // written answers are the answer itself
    assert_eq!(answer_text(&fill, Some("القاهرة")).as_deref(), Some("القاهرة"));
    assert_eq!(answer_text(&short, Some("  فقرة  ")).as_deref(), Some("  فقرة  "), "as written, not trimmed");
    // nothing answered
    for q in [&single, &multi, &tf, &fill, &short] {
        assert_eq!((answer_text(q, None), answer_text(q, Some("")), answer_text(q, Some("  "))), (None, None, None), "{}", q.id);
    }
}

#[test]
fn the_review_shows_original_order_option_text_even_when_the_student_saw_the_options_shuffled() {
    let w = world();
    let id = exam(&w, json!({"shuffle_options": true, "shuffle_questions": true}));
    for seed in 0..8 {
        let who = enroll(&w, &format!("sh{seed}@x.com"));
        let s = start_attempt_with(&w.conn, &who, &id, NOW, &mut StdRng::seed_from_u64(seed)).unwrap();
        let shown = |qid: &str| s.questions.iter().find(|q| q.id == qid).unwrap().options.clone();
        let letter = |qid: &str, text: &str| (b'A' + shown(qid).iter().position(|t| t == text).unwrap() as u8) as char;
        // the student ticks, on their shuffled screen, the options whose TEXT is o1-3 and (for the multiple choice) o2-3, o2-1
        let answers = ans(&[("q1", &letter("q1", "o1-3").to_string()), ("q2", &["o2-3", "o2-1"].iter().map(|t| letter("q2", t)).collect::<String>()), ("q3", "B")]);
        let r = submit_attempt(&w.conn, &who, &s.attempt_id, &answers, NOW + MIN).unwrap();
        let items = r.items.unwrap();
        let item = |qid: &str| items.iter().find(|i| i.id == qid).unwrap();
        assert_eq!(item("q1").your_answer_text.as_deref(), Some("o1-3"), "seed {seed}");
        assert_eq!(item("q2").your_answer_text.as_deref(), Some("o2-1 | o2-3"), "seed {seed}: original order, not the order they saw");
        assert_eq!(item("q2").your_answer.as_deref(), Some("BD"), "…and the stored letters are the original ones");
        assert_eq!(item("q3").your_answer_text.as_deref(), Some("خطأ"));
        assert_eq!(item("q4").your_answer_text, None, "not answered");
        assert_eq!(item("q5").your_answer_text, None);
    }
}

#[test]
fn items_carry_the_chapter_and_difficulty_only_where_the_item_itself_is_shown() {
    let w = world();
    let id = exam(&w, json!({}));
    let at = take(&w, &w.student, &id, NOW, MIN, &all_right());
    let r = attempt_result(&w.conn, &w.student, &at).unwrap();
    let items = r.items.as_ref().unwrap();
    let get = |qid: &str| items.iter().find(|i| i.id == qid).unwrap();
    assert_eq!((get("q1").chapter.as_deref(), get("q1").difficulty.as_deref()), (Some("ف1"), Some("easy")));
    assert_eq!((get("q2").chapter.as_deref(), get("q2").difficulty.as_deref()), (Some("ف1"), Some("medium")));
    assert_eq!((get("q3").chapter.as_deref(), get("q3").difficulty.as_deref()), (Some("ف2"), Some("hard")));
    assert_eq!((get("q4").chapter.as_deref(), get("q4").difficulty.as_deref()), (Some("ف2"), None), "a question without a difficulty has none");
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["items"][0]["difficulty"], json!("easy"));
    assert!(json["items"][0].get("chapter").is_some() && json["items"][0].get("your_answer_text").is_some(), "the fields are on the wire");
    // an exam that hides its answers shows the student no items, hence no chapters and no key
    let hidden = exam(&w, json!({"title": "مخفي", "show_answers": false}));
    let at2 = take(&w, &w.student, &hidden, NOW, MIN, &all_right());
    let student_view = serde_json::to_string(&attempt_result(&w.conn, &w.student, &at2).unwrap()).unwrap();
    assert!(!student_view.contains("ف1") && !student_view.contains("easy") && !student_view.contains("القاهرة"), "{student_view}");
    // …while the grader sees them
    assert!(attempt_result(&w.conn, &w.admin, &at2).unwrap().items.unwrap().iter().any(|i| i.chapter.as_deref() == Some("ف1")));
    // and an embargoed result carries none either
    let held = exam(&w, json!({"title": "مؤجل", "release_mode": "after_close", "closes_at": NOW + 5 * H}));
    let at3 = take(&w, &w.student, &held, NOW, MIN, &all_right());
    let withheld = serde_json::to_string(&attempt_result_at(&w.conn, &w.student, &at3, NOW + 2 * MIN).unwrap()).unwrap();
    assert!(!withheld.contains("ف1") && !withheld.contains("إجابتي"), "{withheld}");
}

// ───────── attempt number, best badge, duration ─────────

#[test]
fn each_attempt_knows_its_number_and_the_best_fully_graded_one_is_marked_newest_on_a_tie() {
    let w = world();
    let id = exam(&w, json!({"max_attempts": 4}));
    let other = enroll(&w, "o@x.com");
    // the classmate's attempts never change my numbers
    take(&w, &other, &id, NOW + 5, MIN, &all_right());
    let a1 = take(&w, &w.student, &id, NOW + 10 * MIN, MIN, &objective_right()); // 5 of 9, fully graded
    let a2 = take(&w, &w.student, &id, NOW + 20 * MIN, MIN, &all_right()); // 5 + a written answer waiting
    let a3 = take(&w, &w.student, &id, NOW + 30 * MIN, MIN, &objective_right()); // 5 again
    let view = |who: &User, at: &str| attempt_result(&w.conn, who, at).unwrap();
    let flags = |who: &User| [&a1, &a2, &a3].map(|a| {
        let v = view(who, a);
        (v.attempt_no, v.is_best)
    });
    // a2 is not fully graded, so it cannot be "best"; a1 and a3 tie on 5, the newest (a3) wins
    assert_eq!(flags(&w.student), [(1, false), (2, false), (3, true)]);
    assert_eq!(flags(&w.admin), [(1, false), (2, false), (3, true)], "graders see the same numbers and badge");
    // grading the written answer makes a2 fully graded with 9: it is the best now
    let g = GradeReq { grades: [("q5".to_string(), 4.0)].into(), ..Default::default() };
    grade_attempt(&w.conn, &w.admin, &a2, &g).unwrap();
    assert_eq!(flags(&w.student), [(1, false), (2, true), (3, false)]);
    assert_eq!(best_graded_attempt(&w.conn, &w.student.id, &id).unwrap().as_deref(), Some(a2.as_str()));
    assert_eq!(best_graded_attempt(&w.conn, &enroll(&w, "nobody@x.com").id, &id).unwrap(), None, "no attempt, no best");
}

#[test]
fn a_withheld_result_is_never_singled_out_as_the_best() {
    let w = world();
    let c = NOW + H;
    let id = exam(&w, json!({"release_mode": "after_close", "closes_at": c, "max_attempts": 2}));
    let low = take(&w, &w.student, &id, NOW, MIN, &ans(&[("q1", "B")]));
    let high = take(&w, &w.student, &id, NOW + 5 * MIN, MIN, &objective_right());
    for (now, best) in [(c + GRACE_MS, [false, false]), (c + GRACE_MS + 1, [false, true])] {
        let flags = [&low, &high].map(|a| attempt_result_at(&w.conn, &w.student, a, now).unwrap().is_best);
        assert_eq!(flags, best, "at {now}");
    }
    assert_eq!(attempt_result_at(&w.conn, &w.student, &low, NOW).unwrap().attempt_no, 1, "the number is not a secret");
}

#[test]
fn the_duration_is_whole_seconds_between_start_and_hand_in_and_absent_for_an_expired_attempt() {
    let w = world();
    let id = exam(&w, json!({"duration_min": 10, "max_attempts": 2}));
    let a = take(&w, &w.student, &id, NOW, 90_999, &objective_right());
    assert_eq!(attempt_result(&w.conn, &w.student, &a).unwrap().duration_sec, Some(90), "floor of 90.999 s");
    // the second attempt is never touched: it expires
    let b = start_attempt(&w.conn, &w.student, &id, NOW + 20 * MIN).unwrap().attempt_id;
    let r = attempt_result_at(&w.conn, &w.student, &b, NOW + 40 * MIN).unwrap();
    assert_eq!((r.status.as_str(), r.duration_sec, r.submitted_at, r.attempt_no), ("expired", None, None, 2));
    // settled from an autosave: stamped at the end, so the duration is the exam's own
    let c = enroll(&w, "c@x.com");
    let cs = start_attempt(&w.conn, &c, &id, NOW).unwrap();
    save_answers(&w.conn, &c, &cs.attempt_id, &ans(&[("q1", "B")]), NOW + MIN).unwrap();
    assert_eq!(attempt_result_at(&w.conn, &c, &cs.attempt_id, NOW + H).unwrap().duration_sec, Some(600));
}

// ───────── fill-in-the-blank normalisation ─────────

/// The vectors the browser's `utils/answerCheck.ts` is tested against too.
const VECTORS: &str = include_str!("../../../shared/fixtures/fill-blank-vectors.json");

fn vectors() -> Vec<(String, String, bool, String)> {
    let v: Vec<Value> = serde_json::from_str(VECTORS).expect("the shared fixture is valid JSON");
    v.iter()
        .map(|x| (x["answer"].as_str().unwrap().to_string(), x["given"].as_str().unwrap().to_string(), x["correct"].as_bool().unwrap(), x["note"].as_str().unwrap_or("").to_string()))
        .collect()
}

fn fill(answer: &str) -> Question {
    question(json!({"id": "f", "type": "fill_blank", "stem": "س", "answer": answer}))
}

#[test]
fn the_shared_fill_blank_vectors_all_hold() {
    let all = vectors();
    assert!(all.len() >= 25, "the fixture has at least 25 vectors, found {}", all.len());
    assert!(all.iter().any(|v| !v.2) && all.iter().any(|v| v.2), "both outcomes are covered");
    for (answer, given, correct, note) in &all {
        let q = fill(answer);
        assert_eq!(auto_grade(&q, Some(given)), Some(*correct), "{note}: key {answer:?} given {given:?}");
        assert_eq!(grade_one(&q, Some(given)).correct, Some(*correct), "{note} (the marking)");
    }
}

#[test]
fn norm_fill_brings_both_sides_to_one_form() {
    let n = norm_fill;
    assert_eq!(n("  ( مِصْر )  . "), "مصر", "marks, punctuation and spaces at the ends");
    assert_eq!(n("٣٫٥ ٪"), "3.5 ٪", "digits and the decimal separator; a percent sign is not touched");
    assert_eq!(n("۱۲۳"), "123");
    assert_eq!(n("١٬٠٠٠"), "1,000");
    assert_eq!(n("نهر\u{00A0}\u{00A0}النيل\t"), "نهر النيل");
    assert_eq!(n("\u{200F}الق\u{200B}اهرة\u{200E}"), "القاهرة");
    assert_eq!(n("ابن-سينا"), "ابن-سينا", "punctuation inside stays");
    assert_eq!(n("مدرسة"), "مدرسة");
    assert_ne!(n("مدرسة"), n("مدرسه"), "teh marbuta and heh are NOT folded");
    assert_eq!(n("..."), "", "nothing but punctuation is nothing");
    assert_eq!(n(""), "");
    assert_eq!(n("Cairo"), "cairo");
    // idempotent: normalising twice changes nothing
    for (a, g, _, _) in vectors() {
        assert_eq!(n(&n(&g)), n(&g), "{g:?}");
        assert_eq!(n(&n(&a)), n(&a), "{a:?}");
    }
}

#[test]
fn the_sign_of_a_number_and_a_hyphen_glued_to_a_word_are_part_of_the_answer() {
    let n = norm_fill;
    assert_eq!(n("-5"), "-5");
    assert_eq!(n("\u{2212}5"), "-5", "the true minus sign is a hyphen");
    assert_eq!(n("(-5)"), "-5");
    assert_eq!(n("-\u{0665}"), "-5", "…in Arabic-Indic digits too");
    assert_eq!(n(".5"), ".5");
    assert_eq!(n("-.5"), "-.5");
    assert_eq!(n("5."), "5");
    assert_eq!(n("- مصر -"), "مصر", "a dash that stands apart is decoration");
    assert_eq!(n("-مصر"), "مصر");
    assert_eq!(n("Cl-"), "cl-");
    assert_eq!(n("(NO3-)."), "no3-");
    let q = |key: &str| fill(key);
    for (key, given, right) in [("-5", "5", false), ("5", "-5", false), ("-5", "-5", true), ("-5", "\u{2212}5", true), (".5", "5", false), ("5", ".5", false), ("Cl-", "Cl", false), ("Cl", "Cl-", false)] {
        assert_eq!(auto_grade(&q(key), Some(given)), Some(right), "key {key:?} given {given:?}");
        assert_eq!(grade_one(&q(key), Some(given)).correct, Some(right), "the marking agrees: key {key:?} given {given:?}");
    }
}

/// A key made only of punctuation cannot be told from an empty key once normalised, and used to be unanswerable; it is
/// compared as typed (the way it always was) instead, and nothing else can match it.
#[test]
fn a_key_of_only_punctuation_is_answered_by_the_same_punctuation() {
    for (key, given, right) in [("?", "?", true), ("?", " ? ", true), ("?", "!", false), ("-", "-", true), ("...", "...", true), ("...", "..", false), ("?", "", false), ("?", "   ", false), ("القاهرة|?", "?", true), ("القاهرة|?", "مصر", false), ("القاهرة", "?", false), ("؟", "؟", true)] {
        assert_eq!(auto_grade(&fill(key), Some(given)), Some(right), "key {key:?} given {given:?}");
    }
    assert_eq!(auto_grade(&fill("?"), None), None, "no answer at all stays unanswered");
}

#[test]
fn a_submitted_attempt_is_marked_with_the_same_normalisation() {
    let w = world();
    let id = exam(
        &w,
        json!({"questions": [
            {"id":"n1","type":"fill_blank","stem":"س","answer":"3.5","score":1},
            {"id":"n2","type":"fill_blank","stem":"س","answer":"مدرسة","score":1},
            {"id":"n3","type":"fill_blank","stem":"س","answer":"القاهرة|مصر","score":1},
            {"id":"n4","type":"fill_blank","stem":"س","answer":"نهر النيل","score":1}
        ]}),
    );
    let r = take(&w, &w.student, &id, NOW, MIN, &ans(&[("n1", "٣٫٥"), ("n2", "مدرسه"), ("n3", " « مصر ». "), ("n4", "نهر\u{00A0}النيل")]));
    let result = attempt_result(&w.conn, &w.student, &r).unwrap();
    let items = result.items.unwrap();
    let ok = |qid: &str| items.iter().find(|i| i.id == qid).unwrap().correct;
    assert_eq!((ok("n1"), ok("n2"), ok("n3"), ok("n4")), (Some(true), Some(false), Some(true), Some(true)));
    assert_eq!(result.score, 3.0);
}
