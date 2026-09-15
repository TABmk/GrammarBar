import { test } from 'node:test';
import assert from 'node:assert/strict';
import { validateCorrection } from '../ui/edits.js';
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
