import { Group, IcosahedronGeometry, Mesh, MeshBasicMaterial, PerspectiveCamera, Scene, TorusGeometry, WebGLRenderer } from 'three';

export function startBootAnimation(canvas: HTMLCanvasElement): () => void {
  const view = canvas.ownerDocument.defaultView!;
  if (!view.WebGL2RenderingContext) return () => {};
  let renderer: WebGLRenderer;
  try { renderer = new WebGLRenderer({ canvas, alpha: true, antialias: true, powerPreference: 'low-power' }); }
  catch {
    console.warn('Could not render the environment boot animation.');
    return () => {};
  }

  const reducedMotion = view.matchMedia('(prefers-reduced-motion: reduce)');
  const scene = new Scene();
  const camera = new PerspectiveCamera(40, 1, 0.1, 10);
  camera.position.z = 3.6;
  const model = new Group();
  const coreGeometry = new IcosahedronGeometry(0.42, 0);
  const coreMaterial = new MeshBasicMaterial({ color: 0xeeeeee, wireframe: true, transparent: true, opacity: 0.7 });
  const core = new Mesh(coreGeometry, coreMaterial);
  const ringGeometry = new TorusGeometry(0.94, 0.008, 4, 96);
  const ringMaterial = new MeshBasicMaterial({ color: 0xbcbcbc, transparent: true, opacity: 0.55, depthWrite: false });
  const rings = [0, 1, 2].map(index => {
    const ring = new Mesh(ringGeometry, ringMaterial);
    ring.rotation.set(0.65 + index * 0.7, index * Math.PI / 3, index * 0.3);
    return ring;
  });
  model.add(core, ...rings);
  scene.add(model);
  let stopped = false;

  function render(time: number): void {
    if (stopped) return;
    model.rotation.x = 0.2 + Math.sin(time * 0.0003) * 0.12;
    model.rotation.y = time * 0.00022;
    core.rotation.y = -time * 0.00045;
    renderer.render(scene, camera);
  }

  function resize(): void {
    if (stopped) return;
    const width = canvas.clientWidth || 200;
    const height = canvas.clientHeight || 200;
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    renderer.setSize(width, height, false);
    renderer.render(scene, camera);
  }

  function updateMotion(): void {
    if (stopped) return;
    renderer.setAnimationLoop(reducedMotion.matches ? null : render);
    render(0);
  }

  renderer.setPixelRatio(Math.min(view.devicePixelRatio, 2));
  const observer = new ResizeObserver(resize);
  observer.observe(canvas);
  reducedMotion.addEventListener('change', updateMotion);
  resize();
  updateMotion();

  return () => {
    if (stopped) return;
    stopped = true;
    renderer.setAnimationLoop(null);
    observer.disconnect();
    reducedMotion.removeEventListener('change', updateMotion);
    coreGeometry.dispose(); ringGeometry.dispose();
    coreMaterial.dispose(); ringMaterial.dispose();
    scene.clear();
    renderer.dispose();
  };
}
