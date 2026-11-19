import userGuide from '../../docs/user-guide.html?raw';

export function initializeHelp(): void {
  const template = document.createElement('template');
  template.innerHTML = userGuide;
  const guide = template.content.querySelector('main')!;
  const heading = guide.querySelector('h1')!;
  heading.id = 'help-heading';
  heading.tabIndex = -1;
  // Repository files are not available in the packaged app.
  for (const link of guide.querySelectorAll('a[href^="../"]')) link.replaceWith(...link.childNodes);
  document.getElementById('help-view')!.replaceChildren(...guide.childNodes);
}
