import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

test('height drag uses screen coordinates, stops on release, and supports keyboard', () => {
  const listeners = {}, calls = [];
  const handle = { addEventListener: (name, fn) => listeners[name] = fn, setPointerCapture() {} };
  const source = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8');
  runInNewContext(source.slice(source.indexOf("for (const handle of document.querySelectorAll('.resize-handle'))")), {
    document: { querySelectorAll: () => [handle] }, innerHeight: 380,
    invoke: (_, args) => { calls.push(args.height); return Promise.resolve(); },
  });
  const event = { button: 0, screenY: 400, pointerId: 1, preventDefault() {} };
  listeners.pointerdown(event);
  listeners.pointermove({ screenY: 550 });
  assert.equal(calls.at(-1), 530);
  listeners.pointerup();
  listeners.pointermove({ screenY: 600 });
  assert.equal(calls.length, 1);
  listeners.pointerdown(event);
  listeners.lostpointercapture();
  listeners.pointermove({ screenY: 600 });
  assert.equal(calls.length, 1);
  listeners.keydown({ key: 'ArrowDown', preventDefault() {} });
  assert.equal(calls.at(-1), 420);
  listeners.keydown({ key: 'ArrowUp', preventDefault() {} });
  assert.equal(calls.at(-1), 340);
});
