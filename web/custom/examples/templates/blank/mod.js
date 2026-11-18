// __MOD_NAME__
alc.define('__MOD_ID__', (mod) => {
  mod.addCommand({ id: 'hello', title: '__MOD_NAME__: say hello', run: () => alc.toast('Hello from __MOD_NAME__') });
});
