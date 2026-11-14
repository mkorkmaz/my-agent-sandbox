type ModalState = { phase: 'none' } | { phase: 'active'; dialog: HTMLDialogElement };
type DialogExposure = 'active' | 'obscured' | 'closed';

class ModalStack {
  private state: ModalState = { phase: 'none' };
  private returnFocusIds = new WeakMap<HTMLDialogElement, string>();

  expose(): void {
    for (const dialog of document.querySelectorAll<HTMLDialogElement>('dialog')) this.exposeDialog(dialog);
  }

  show(dialog: HTMLDialogElement, returnFocusId?: string): void {
    if (dialog.open) return;
    if (returnFocusId) this.returnFocusIds.set(dialog, returnFocusId);
    // Keep DOM order aligned with the native modal stack for WebKit accessibility.
    document.body.append(dialog);
    this.state = { phase: 'active', dialog };
    this.expose();
    dialog.showModal();
  }

  closed(dialog: HTMLDialogElement): void {
    if (dialog.tagName !== 'DIALOG' || dialog.open) return;
    const active = [...document.querySelectorAll<HTMLDialogElement>('dialog[open]')].at(-1) ?? null;
    this.state = active ? { phase: 'active', dialog: active } : { phase: 'none' };
    this.expose();
    const id = this.returnFocusIds.get(dialog);
    this.returnFocusIds.delete(dialog);
    const target = id ? document.getElementById(id) : null;
    if (!target || target.closest('[hidden], [inert]')) return;
    if (active && !active.contains(target)) return;
    target.focus({ preventScroll: true });
  }

  private exposure(dialog: HTMLDialogElement): DialogExposure {
    if (this.state.phase === 'active' && dialog === this.state.dialog) return 'active';
    return dialog.open ? 'obscured' : 'closed';
  }

  private exposeDialog(dialog: HTMLDialogElement): void {
    const exposure = this.exposure(dialog);
    dialog.inert = exposure !== 'active';
    if (exposure === 'active') {
      dialog.removeAttribute('aria-hidden');
      dialog.setAttribute('aria-modal', 'true');
      return;
    }
    dialog.removeAttribute('aria-modal');
    dialog.setAttribute('aria-hidden', 'true');
  }
}

const modalStack = new ModalStack();
modalStack.expose();
document.addEventListener('close', event => { modalStack.closed(event.target as HTMLDialogElement); }, true);

export function showDialog(dialog: HTMLDialogElement, returnFocusId?: string): void {
  modalStack.show(dialog, returnFocusId);
}
