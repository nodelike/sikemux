const REGEX = /^\/(.*)\/([a-z]*)$/s;

export function toolMatches(pattern, name) {
  if (pattern === undefined) return true;
  if (Array.isArray(pattern)) return pattern.some((one) => toolMatches(one, name));
  if (pattern.endsWith('*')) return name.startsWith(pattern.slice(0, -1));
  return pattern === name;
}

export function valueMatches(expected, actual) {
  if (expected && typeof expected === 'object' && !Array.isArray(expected)) {
    if ('$present' in expected) return (actual !== undefined) === expected.$present;
    if ('$not' in expected) return !valueMatches(expected.$not, actual);
    if ('$any' in expected) {
      const { $any, ...also } = expected;
      return $any.some((one) => valueMatches(one, actual)) && (Object.keys(also).length === 0 || valueMatches(also, actual));
    }
    if (!actual || typeof actual !== 'object') return false;
    return Object.entries(expected).every(([key, value]) => valueMatches(value, actual[key]));
  }
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual)) return false;
    return expected.every((item) => actual.some((one) => valueMatches(item, one)));
  }
  if (typeof expected === 'string') {
    const regex = expected.match(REGEX);
    if (regex) {
      const text = typeof actual === 'string' ? actual : JSON.stringify(actual ?? '');
      return new RegExp(regex[1], regex[2]).test(text);
    }
  }
  return expected === actual;
}

export function callMatches(matcher, call) {
  if (matcher.any) return matcher.any.some((one) => callMatches(one, call));
  return toolMatches(matcher.tool, call.tool) && (!matcher.args || valueMatches(matcher.args, call.args));
}

export function describe(matcher) {
  if (matcher.any) return matcher.any.map(describe).join(' or ');
  const args = matcher.args ? ` ${JSON.stringify(matcher.args)}` : '';
  return `${[matcher.tool].flat().join('|')}${args}`;
}
