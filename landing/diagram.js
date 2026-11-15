const map = document.querySelector('.sandboxMap');
const traces = map.querySelectorAll('.mapTrace');
const motionPreference = matchMedia('(prefers-reduced-motion: reduce)');
const firstTraceDelay = 150;
const traceInterval = 1800;
const travelDuration = 1400;
const arrivalDelay = travelDuration * 0.75;
const pulseFadeIn = 100;
const pulseHold = 900;
const pulseFadeOut = 200;
const pulseDuration = pulseFadeIn + pulseHold + pulseFadeOut;
const holdDuration = 3000;
const lastTraceStart = firstTraceDelay + (traces.length - 1) * traceInterval;
const cycleDuration = lastTraceStart + Math.max(travelDuration, arrivalDelay + pulseDuration) + holdDuration;
const loopTiming = { duration: cycleDuration, iterations: Infinity };
const animations = typeof map.animate === 'function' ? Array.from(traces).flatMap(animateTrace) : [];
let inView = false;

function animateTrace(trace, index) {
  const start = firstTraceDelay + index * traceInterval;
  const arrival = start + arrivalDelay;
  const travel = trace.animate([
    { strokeDashoffset: '5', opacity: 0, offset: start / cycleDuration },
    { strokeDashoffset: '0', opacity: 1, offset: (start + travelDuration * 0.12) / cycleDuration, easing: 'ease-in-out' },
    { strokeDashoffset: '-96', opacity: 1, offset: arrival / cycleDuration },
    { strokeDashoffset: '-96', opacity: 0, offset: (start + travelDuration) / cycleDuration },
  ], loopTiming);
  const contact = document.getElementById(trace.dataset.contact);
  const frames = [
    { opacity: 0, offset: 0 },
    { opacity: 0, offset: arrival / cycleDuration },
    { opacity: 1, offset: (arrival + pulseFadeIn) / cycleDuration },
    { opacity: 1, offset: (arrival + pulseFadeIn + pulseHold) / cycleDuration },
    { opacity: 0, offset: (arrival + pulseDuration) / cycleDuration },
    { opacity: 0, offset: 1 },
  ];
  // Both agents can pulse the same project tile.
  return [travel, contact.animate(frames, { ...loopTiming, composite: 'add' })];
}

function updatePlayback() {
  if (motionPreference.matches) {
    animations.forEach(animation => animation.cancel());
    return;
  }
  if (!inView || document.hidden) {
    animations.forEach(animation => animation.pause());
    return;
  }
  animations.forEach(animation => animation.play());
}

if (animations.length) {
  motionPreference.addEventListener('change', updatePlayback);
  document.addEventListener('visibilitychange', updatePlayback);
  const observer = new IntersectionObserver(([entry]) => {
    inView = entry.intersectionRatio >= 0.5;
    updatePlayback();
  }, { threshold: 0.5 });
  observer.observe(map);
  updatePlayback();
}
