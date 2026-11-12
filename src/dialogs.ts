const returnFocusIds = new WeakMap<HTMLDialogElement, string>();

function exposeDialog(active: HTMLDialogElement | null): void {
  for (const dialog of document.querySelectorAll<HTMLDialogElement>('dialog')) {
    dialog.inert = dialog !== active;
    if (dialog === active) {
      dialog.removeAttribute('aria-hidden');
      dialog.setAttribute('aria-modal', 'true');
    } else {
      dialog.removeAttribute('aria-modal');
      dialog.setAttribute('aria-hidden', 'true');
    }
  }
}

exposeDialog(null);
document.addEventListener('close', event => {
  const closed = event.target as HTMLDialogElement;
  if (closed.tagName !== 'DIALOG' || closed.open) return;
  const active = [...document.querySelectorAll<HTMLDialogElement>('dialog[open]')].at(-1) ?? null;
  exposeDialog(active);
  const id = returnFocusIds.get(closed);
  returnFocusIds.delete(closed);
  const target = id ? document.getElementById(id) : null;
  if (target && !target.closest('[hidden], [inert]') && (!active || active.contains(target))) {
    target.focus({ preventScroll: true });
  }
}, true);

export function showDialog(dialog: HTMLDialogElement, returnFocusId?: string): void {
  if (dialog.open) return;
  if (returnFocusId) returnFocusIds.set(dialog, returnFocusId);
  // Keep DOM order aligned with the native modal stack for WebKit accessibility.
  document.body.append(dialog);
  exposeDialog(dialog);
  dialog.showModal();
}
