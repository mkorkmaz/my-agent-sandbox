export function setBusy(element: HTMLElement, busy: boolean): void {
  element.classList.toggle('is-busy', busy);
  element.setAttribute('aria-busy', String(busy));
}

export function setLoading(element: HTMLElement, loading: boolean): void {
  element.classList.toggle('is-loading', loading);
  element.setAttribute('aria-busy', String(loading));
}

export function renderSkeleton(container: HTMLElement, columns = 1): void {
  const table = container.tagName === 'TBODY';
  const rowTag = table ? 'tr' : container.tagName === 'UL' ? 'li' : 'span';
  const rows = Array.from({ length: 3 }, () => {
    const row = document.createElement(rowTag);
    row.className = 'skeleton-row'; row.setAttribute('aria-hidden', 'true');
    for (let column = 0; column < columns; column++) {
      const cell = document.createElement(table ? 'td' : 'span');
      const line = document.createElement('span'); line.className = 'skeleton';
      cell.append(line); row.append(cell);
    }
    return row;
  });
  container.replaceChildren(...rows);
}
