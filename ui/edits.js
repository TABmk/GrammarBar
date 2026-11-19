// The Rust side aligns model output with the source; this re-checks the contract
// before anything is rendered.
export function validateCorrection(source, value) {
  if (!value || !Array.isArray(value.segments)) throw new Error('Invalid response.');
  if (value.segments.some(s => !s || ['original', 'replacement', 'explanation'].some(k => typeof s[k] !== 'string')
    || (!s.original && !s.replacement) || (s.original !== s.replacement && !s.explanation.trim()))) throw new Error('Invalid edits.');
  if (value.segments.map(s => s.original).join('') !== source) throw new Error('Edits do not match the original text.');
  const failed = Number.isInteger(value.failed) ? value.failed : 0, parts = Number.isInteger(value.parts) ? value.parts : 1;
  return { segments: value.segments, text: value.segments.map(s => s.replacement).join(''),
    count: value.segments.filter(s => s.original !== s.replacement).length, failed, parts };
}

// Whitespace-only edits are invisible as plain text, so show them with markers.
export function visible(text, inserted) {
  if (!/^\s+$/u.test(text)) return text;
  return text.replace(/[^\S\n]/gu, c => c === '\t' ? '⇥' : '␣').replace(/\n/g, inserted ? '¶\n' : '¶');
}
