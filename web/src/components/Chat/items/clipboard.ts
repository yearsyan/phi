/** Writes text through the Clipboard API with a legacy DOM fallback. */
export async function writeClipboard(text: string): Promise<void> {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Fall through for browsers that expose Clipboard but deny the call.
    }
  }

  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  try {
    if (
      typeof document.execCommand !== 'function' ||
      !document.execCommand('copy')
    ) {
      throw new Error('copy command was rejected');
    }
  } finally {
    textarea.remove();
  }
}
