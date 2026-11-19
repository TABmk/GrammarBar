import { test } from 'node:test';
import assert from 'node:assert/strict';
import { validateCorrection, visible } from '../ui/edits.js';
test('lossless Unicode, insertion, deletion, repeated words and unchanged spans', () => {
  const segments = [
    { original: '🙂 I ', replacement: '🙂 I ', explanation: '' },
    { original: 'has', replacement: 'have', explanation: 'Agreement' },
    { original: ' tea tea', replacement: ' tea', explanation: 'Repetition' },
    { original: '', replacement: '.', explanation: 'Punctuation' },
    { original: '\n', replacement: '\n', explanation: '' },
  ];
  const result = validateCorrection('🙂 I has tea tea\n', { segments });
  assert.equal(result.text, '🙂 I have tea.\n'); assert.equal(result.count, 3);
  assert.throws(() => validateCorrection('Different text', { segments }));
  assert.throws(() => validateCorrection('a', { segments: [{ original:'a', replacement:'b', explanation:'' }] }));
  assert.throws(() => validateCorrection('a', { segments: [{ original:'a', replacement:5, explanation:'' }] }));
});
test('unchanged text and markup are preserved as plain text', () => {
  const source = '<script>alert("x")</script>\n café e\u0301';
  const result = validateCorrection(source, { segments: [{original:source,replacement:source,explanation:''}] });
  assert.equal(result.text, source); assert.equal(result.count, 0);
});
test('partial failures are reported', () => {
  const result = validateCorrection('a', { segments: [{ original: 'a', replacement: 'a', explanation: '' }], failed: 1, parts: 3 });
  assert.equal(result.failed, 1); assert.equal(result.parts, 3);
});
test('whitespace-only edits get visible markers', () => {
  assert.equal(visible('  ', false), '␣␣');
  assert.equal(visible('\n', true), '¶\n');
  assert.equal(visible('\n\t', false), '¶⇥');
  assert.equal(visible(' a ', true), ' a ');
});
test('responses that break the contract are rejected', () => {
  assert.throws(() => validateCorrection('a', null));
  assert.throws(() => validateCorrection('a', { segments: 'a' }));
  assert.throws(() => validateCorrection('a', { segments: [null] }));
  assert.throws(() => validateCorrection('', { segments: [{ original: '', replacement: '', explanation: '' }] }));
  assert.throws(() => validateCorrection('a', { segments: [{ original: 'a', replacement: 'b', explanation: '   ' }] }));
});
test('missing part counts default to one complete part', () => {
  const result = validateCorrection('a', { segments: [{ original: 'a', replacement: 'b', explanation: 'x' }], failed: '2', parts: null });
  assert.equal(result.failed, 0); assert.equal(result.parts, 1); assert.equal(result.count, 1); assert.equal(result.text, 'b');
});
