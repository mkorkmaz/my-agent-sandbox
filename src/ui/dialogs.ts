function nativeDialog(dialog: HTMLElement): dialog is HTMLDialogElement { return dialog.tagName === 'DIALOG'; }

export function dialogOpen(dialog: HTMLElement): boolean { return nativeDialog(dialog) ? dialog.open : !dialog.hidden; }

export function hasOpenDialog(): boolean { return document.querySelector('dialog[open], [data-inline-modal]:not([hidden])') !== null; }

export function closeDialog(dialog: HTMLElement): void {
  if (nativeDialog(dialog)) { dialog.close(); return; }
  dialog.hidden = true;
  dialog.dispatchEvent(new (document.defaultView!.Event)('close'));
}

class ModalStack {
  private dialogs: HTMLElement[] = [];
  private returnFocusIds = new WeakMap<HTMLElement, string>();

  show(dialog: HTMLElement, returnFocusId?: string): void {
    if (dialogOpen(dialog)) return;
    this.returnFocusIds.delete(dialog);
    if (returnFocusId) this.returnFocusIds.set(dialog, returnFocusId);
    this.dialogs = this.dialogs.filter(candidate => candidate !== dialog);
    this.dialogs.push(dialog);
    if (!nativeDialog(dialog)) {
      dialog.hidden = false;
      this.updateInlineModal();
      tabStops(dialog)[0]?.focus({ preventScroll: true });
      return;
    }
    dialog.showModal();
    this.updateInlineModal();
  }

  closed(dialog: HTMLElement): void {
    if (!this.dialogs.includes(dialog) || dialogOpen(dialog)) return;
    this.dialogs = this.dialogs.filter(candidate => dialogOpen(candidate));
    this.updateInlineModal();
    const active = this.dialogs.at(-1);
    const id = this.returnFocusIds.get(dialog);
    this.returnFocusIds.delete(dialog);
    const target = id ? document.getElementById(id) : null;
    if (!target || target.closest('[hidden], [inert]')) return;
    if (active && !active.contains(target)) return;
    target.focus({ preventScroll: true });
  }

  keydown(event: KeyboardEvent): void {
    const active = this.activeInlineModal();
    if (!active || event.defaultPrevented || event.isComposing || event.altKey || event.metaKey || event.ctrlKey) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      const cancel = new (document.defaultView!.Event)('cancel', { cancelable: true });
      if (active.dispatchEvent(cancel)) closeDialog(active);
      return;
    }
    if (event.key !== 'Tab') return;
    event.preventDefault();
    const stops = tabStops(active);
    const index = stops.indexOf(document.activeElement as HTMLElement);
    const next = event.shiftKey ? (index <= 0 ? stops.length - 1 : index - 1) : (index + 1) % stops.length;
    stops[next]?.focus();
  }

  containFocus(event: FocusEvent): void {
    const active = this.activeInlineModal();
    if (active && !active.contains(event.target as Node)) tabStops(active)[0]?.focus({ preventScroll: true });
  }

  private activeInlineModal(): HTMLElement | undefined {
    const active = this.dialogs.at(-1);
    return active?.hasAttribute('data-inline-modal') && dialogOpen(active) ? active : undefined;
  }

  private updateInlineModal(): void {
    const panel = document.querySelector<HTMLElement>('[data-inline-modal]');
    if (!panel) return;
    const open = dialogOpen(panel);
    panel.toggleAttribute('inert', open && this.dialogs.at(-1) !== panel);
    // Keep the settings panel in WebKit's document tree. Only its background branches are inert.
    for (let foreground: Element | null = panel; foreground?.parentElement && foreground.parentElement !== document.body; foreground = foreground.parentElement) {
      for (const sibling of foreground.parentElement.children) {
        if (sibling !== foreground) sibling.toggleAttribute('inert', open);
      }
    }
  }
}

function tabStops(dialog: HTMLElement): HTMLElement[] {
  return [...dialog.querySelectorAll<HTMLElement>('button, input, select, textarea, a[href], summary, [tabindex]')]
    .filter(element => {
      if (element.matches(':disabled, [type="hidden"]') || element.closest('[hidden], [inert]')) return false;
      if (element.tabIndex < 0 && !(element.tagName === 'SUMMARY' && !element.hasAttribute('tabindex'))) return false;
      const closed = element.closest<HTMLDetailsElement>('details:not([open])');
      return !closed || (closed.querySelector(':scope > summary') === element && !closed.parentElement?.closest('details:not([open])'));
    });
}

const modalStack = new ModalStack();
document.addEventListener('close', event => { modalStack.closed(event.target as HTMLElement); }, true);
document.addEventListener('keydown', event => { modalStack.keydown(event); }, true);
document.addEventListener('focusin', event => { modalStack.containFocus(event); });

export function showDialog(dialog: HTMLElement, returnFocusId?: string): void {
  modalStack.show(dialog, returnFocusId);
}
