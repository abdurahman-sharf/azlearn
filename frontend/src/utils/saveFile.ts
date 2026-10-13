/**
 * Saves a blob through a temporary link with an ASCII file name (some browsers ignore `download` on detached anchors, so
 * the link is attached for the click; the object URL is released a little later because a download may still be reading it).
 */
export function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  a.style.display = 'none'
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}
