// Run with: node --disable-proto=delete --test packaging/web-ext/security.test.cjs
// web-ext's addons-linter still depends on deepmerge 4.3.1. Exercise the
// prototype-poisoning bypasses and require the same runtime guard as the CI.
const assert = require('node:assert/strict');
const test = require('node:test');
const merge = require('deepmerge');
const ExtendableError = require('es6-error');

const payload = () => JSON.parse('{"__proto__":{"auditPoisoned":true}}');

test('the web-ext runtime has no prototype accessor', () => {
  assert.equal(Object.getOwnPropertyDescriptor(Object.prototype, '__proto__'), undefined);
  assert.equal(({}).__proto__, undefined);
});

test('deepmerge cannot poison a returned object through a null-prototype target', () => {
  assert.equal(merge(Object.create(null), payload()).auditPoisoned, undefined);
  assert.equal({}.auditPoisoned, undefined);
});

test('deepmerge cannot poison a returned object while copying its target', () => {
  assert.equal(merge(payload(), {}).auditPoisoned, undefined);
  assert.equal({}.auditPoisoned, undefined);
});

test('ordinary nested merges used by the linter still work', () => {
  assert.deepEqual(merge({ flags: { first: true } }, { flags: { second: true } }), {
    flags: { first: true, second: true },
  });
});

// `--disable-proto=throw` broke this: es6-error reads `__proto__`, so every error of web-ext (a
// refused upload) ended as ERR_PROTO_ACCESS, its real reason lost.
test("web-ext's errors keep their message", () => {
  class WebExtError extends ExtendableError {}
  assert.equal(new WebExtError('Upload failed: 401').message, 'Upload failed: 401');
});
