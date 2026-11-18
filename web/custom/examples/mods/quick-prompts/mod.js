// Quick prompts: canned replies a phone keyboard makes slow to type.
// The key bar and the palette get the built-in four; the widget takes its
// own list from its options ("Label | text" per line), set in the studio.
alc.define('quick-prompts', (mod) => {
  const zh = alc.locale === 'zh-TW';
  const PROMPTS = [
    { id: 'continue', label: zh ? '繼續' : 'Continue', text: 'continue' },
    { id: 'tests', label: zh ? '跑測試' : 'Run tests', text: 'Run the test suite and fix anything that fails.' },
    { id: 'summary', label: zh ? '摘要' : 'Summarise', text: 'Summarise what you changed so far, briefly.' },
    { id: 'compact', label: '/compact', text: '/compact' },
  ];

  function send(text) {
    if (!alc.current()) return alc.toast(zh ? '請先開啟 session' : 'Open a session first', 'warn');
    if (alc.type(text)) alc.key('\r');
    return undefined;
  }

  // "Label | text" per line; a line without a bar is both.
  function parse(lines) {
    return String(lines || '')
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const [label, ...rest] = line.split('|');
        const text = rest.join('|').trim() || label.trim();
        return { label: label.trim(), text };
      });
  }

  for (const prompt of PROMPTS) {
    mod.addKey({ label: prompt.label, run: () => send(prompt.text) });
    mod.addCommand({
      id: prompt.id,
      title: `${zh ? '送出' : 'Send'} “${prompt.label}”`,
      hint: prompt.text,
      run: () => send(prompt.text),
    });
  }

  mod.addWidget({
    id: 'buttons',
    title: zh ? '快速提示' : 'Quick prompts',
    description: zh ? '一排可自訂的提示按鈕。' : 'A row of prompt buttons you can edit.',
    icon: 'bolt',
    slots: ['view.top', 'view.bottom', 'view.left', 'view.right', 'float'],
    options: [
      {
        key: 'prompts',
        type: 'textarea',
        label: zh ? '提示（每行「標籤 | 內容」）' : 'Prompts ("Label | text" per line)',
        default: PROMPTS.map((prompt) => `${prompt.label} | ${prompt.text}`).join('\n'),
      },
      { key: 'enter', type: 'boolean', label: zh ? '貼上後按 Enter' : 'Press Enter after pasting', default: true },
    ],
    render(root, ctx) {
      const list = parse(ctx.options.prompts);
      root.append(alc.h('div', { class: 'qp-row' }, list.map((prompt) => alc.h('button', {
        type: 'button',
        class: 'btn small',
        title: prompt.text,
        disabled: !alc.canType,
        onclick: () => {
          if (!alc.current()) return alc.toast(zh ? '請先開啟 session' : 'Open a session first', 'warn');
          if (alc.type(prompt.text) && ctx.options.enter) alc.key('\r');
          return undefined;
        },
      }, prompt.label))));
    },
  });
});
