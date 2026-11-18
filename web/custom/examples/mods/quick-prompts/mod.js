// Quick prompts: canned replies a phone keyboard makes slow to type.
// Edit PROMPTS to taste. Each one is pasted into the open session and sent.
alc.define('quick-prompts', (mod) => {
  const PROMPTS = [
    { id: 'continue', label: 'Continue', text: 'continue' },
    { id: 'tests', label: 'Run tests', text: 'Run the test suite and fix anything that fails.' },
    { id: 'summary', label: 'Summarise', text: 'Summarise what you changed so far, briefly.' },
    { id: 'compact', label: '/compact', text: '/compact' },
  ];

  function send(text) {
    if (!alc.current()) return alc.toast('Open a session first', 'warn');
    if (alc.type(text)) alc.key('\r');
  }

  for (const prompt of PROMPTS) {
    mod.addKey({ label: prompt.label, run: () => send(prompt.text) });
    mod.addCommand({
      id: prompt.id,
      title: `Send “${prompt.label}”`,
      hint: prompt.text,
      run: () => send(prompt.text),
    });
  }
});
