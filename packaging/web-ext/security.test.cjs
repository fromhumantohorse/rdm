// Run with: node --disable-proto=throw --test packaging/web-ext/security.test.cjs
// web-ext's addons-linter still depends on deepmerge 4.3.1. Exercise the
// prototype-poisoning bypasses and require the same runtime guard as the CI.
const assert = require('node:assert/strict');
const test = require('node:test');
const merge = require('deepmerge');

test('the web-ext runtime rejects prototype access', () => {
  assert.throws(() => ({}).__proto__, { code: 'ERR_PROTO_ACCESS' });
});

test('deepmerge cannot poison a returned object through a null-prototype target', () => {
  const payload = JSON.parse('{"__proto__":{"auditPoisoned":true}}');
  assert.throws(() => merge(Object.create(null), payload), { code: 'ERR_PROTO_ACCESS' });
  assert.equal({}.auditPoisoned, undefined);
});

test('deepmerge cannot poison a returned object while copying its target', () => {
  const payload = JSON.parse('{"__proto__":{"auditPoisoned":true}}');
  assert.throws(() => merge(payload, {}), { code: 'ERR_PROTO_ACCESS' });
  assert.equal({}.auditPoisoned, undefined);
});

test('ordinary nested merges used by the linter still work', () => {
  assert.deepEqual(merge({ flags: { first: true } }, { flags: { second: true } }), {
    flags: { first: true, second: true },
  });
});
