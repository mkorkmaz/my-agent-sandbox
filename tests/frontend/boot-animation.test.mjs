import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { stripTypeScriptTypes } from 'node:module';
import { JSDOM } from 'jsdom';
import * as three from 'three';

async function animationHarness({ reducedMotion = false, webgl = true, failRenderer = false } = {}) {
  const dom = new JSDOM('<canvas></canvas>');
  const canvas = dom.window.document.querySelector('canvas');
  const changes = new Set();
  const preference = { matches: reducedMotion,
    addEventListener: (_event, callback) => changes.add(callback),
    removeEventListener: (_event, callback) => changes.delete(callback) };
  dom.window.matchMedia = () => preference;
  if (webgl) dom.window.WebGL2RenderingContext = class {};
  Object.defineProperty(dom.window, 'devicePixelRatio', { value: 3 });
  Object.defineProperties(canvas, { clientWidth: { value: 240 }, clientHeight: { value: 160 } });
  const observed = { created: 0, frames: 0, disposed: 0, disconnected: 0, warnings: [], loop: null, scene: null, resize: null };
  class Renderer {
    constructor() { observed.created++; if (failRenderer) throw Error('Private driver detail'); }
    setPixelRatio(value) { observed.pixelRatio = value; }
    setSize(width, height) { observed.size = [width, height]; }
    render(scene, camera) { observed.frames++; observed.scene = scene; observed.aspect = camera.aspect; }
    setAnimationLoop(callback) { observed.loop = callback; }
    dispose() { observed.disposed++; }
  }
  const exports = { ...three, WebGLRenderer: Renderer };
  const context = vm.createContext({
    console: { warn: message => observed.warnings.push(message) },
    ResizeObserver: class {
      constructor(callback) { observed.resize = callback; }
      observe(element) { assert.equal(element, canvas); }
      disconnect() { observed.disconnected++; }
    },
  });
  const graphics = new vm.SyntheticModule(Object.keys(exports), function() {
    for (const [name, value] of Object.entries(exports)) this.setExport(name, value);
  }, { context });
  const source = fs.readFileSync(new URL('../../src/terminal/boot-animation.ts', import.meta.url), 'utf8');
  const module = new vm.SourceTextModule(stripTypeScriptTypes(source), { context });
  await module.link(name => { assert.equal(name, 'three'); return graphics; });
  await module.evaluate();
  return { dom, observed, changes, preference, start: () => module.namespace.startBootAnimation(canvas),
    motion: reduced => { preference.matches = reduced; for (const callback of changes) callback(); } };
}

test('boot animation releases shared geometry, materials, frames, observers and listeners when hidden', async t => {
  const harness = await animationHarness();
  t.after(() => harness.dom.window.close());
  const stop = harness.start();
  const { observed } = harness;
  assert.equal(observed.created, 1);
  assert.equal(observed.pixelRatio, 2);
  assert.deepEqual(observed.size, [240, 160]);
  assert.equal(observed.aspect, 1.5);
  const group = observed.scene.children[0];
  assert.equal(group.children.length, 4);
  const resources = new Set(group.children.flatMap(mesh => [mesh.geometry, mesh.material]));
  assert.equal(resources.size, 4);
  let disposals = 0;
  for (const resource of resources) resource.addEventListener('dispose', () => disposals++);
  const frame = observed.loop;
  frame(1000);
  assert.notEqual(group.rotation.y, 0);
  stop(); stop();
  assert.equal(disposals, 4);
  assert.equal(observed.disposed, 1);
  assert.equal(observed.disconnected, 1);
  assert.equal(harness.changes.size, 0);
  assert.equal(observed.loop, null);
  assert.equal(observed.scene.children.length, 0);
  const frames = observed.frames;
  frame(2000); observed.resize(); harness.motion(true);
  assert.equal(observed.frames, frames);
});

test('reduced motion renders a still scene and live preference changes stop or resume animation', async t => {
  const harness = await animationHarness({ reducedMotion: true });
  t.after(() => harness.dom.window.close());
  const stop = harness.start();
  assert.ok(harness.observed.frames > 0);
  assert.equal(harness.observed.loop, null);
  harness.motion(false);
  assert.equal(typeof harness.observed.loop, 'function');
  harness.motion(true);
  assert.equal(harness.observed.loop, null);
  assert.equal(harness.observed.created, 1);
  stop();
});

test('missing WebGL and renderer failure leave status presentation independent of graphics', async t => {
  for (const options of [{ webgl: false }, { failRenderer: true }]) await t.test(JSON.stringify(options), async t => {
    const harness = await animationHarness(options);
    t.after(() => harness.dom.window.close());
    const stop = harness.start();
    assert.equal(typeof stop, 'function');
    assert.doesNotThrow(() => { stop(); stop(); });
    assert.equal(harness.observed.loop, null);
    assert.equal(harness.changes.size, 0);
    assert.equal(harness.observed.disposed, 0);
    assert.equal(harness.observed.warnings.some(message => message.includes('Private driver detail')), false);
  });
});
