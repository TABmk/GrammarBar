export function validateCorrection(source, value) {
  if (!value || !Array.isArray(value.segments) || value.segments.length > 2000) throw new Error('Invalid response.');
  if (value.segments.some(s => !s || ['original', 'replacement', 'explanation'].some(k => typeof s[k] !== 'string')
    || (!s.original && !s.replacement) || (s.original !== s.replacement && !s.explanation.trim()))) throw new Error('Invalid edits.');
  if (value.segments.map(s => s.original).join('') !== source) throw new Error('Edits do not match the original text.');
  return { segments: value.segments, text: value.segments.map(s => s.replacement).join(''),
    count: value.segments.filter(s => s.original !== s.replacement).length };
}
