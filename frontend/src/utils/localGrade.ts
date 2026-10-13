// How the LOCAL practice mode (stores/practice.ts) checks an answer to a choice or fill-in question. Kept out of the store so a
// plain script can test it. (The platform's practice page uses utils/answerCheck.ts, which mirrors the server instead.)

/** The letters A-J of a choice answer, sorted: "c, a" and "AC" read the same. Options go up to J, as in the platform's exams. */
export function choiceLetters(s: string | null | undefined): string {
  return (s ?? '').trim().toUpperCase().replace(/[^A-J]/g, '').split('').sort().join('')
}

/** A choice answer is right when the student chose something and the letters equal the key's. An empty answer is never right. */
export function choiceCorrect(key: string, given: string | null | undefined): boolean {
  const g = choiceLetters(given)
  return g !== '' && g === choiceLetters(key)
}

/**
 * A fill-in answer is right when the text equals the key or one of its `a|b` alternatives (the same alternatives the platform's
 * marking accepts, which a question saved from an exam keeps); case and outer spaces are ignored. An empty answer is never right.
 */
export function fillCorrect(key: string, given: string | null | undefined): boolean {
  const g = (given ?? '').trim().toLowerCase()
  return g !== '' && key.split('|').some((alt) => alt.trim().toLowerCase() === g)
}
