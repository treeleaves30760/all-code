/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    {
      type: 'category',
      label: 'Start',
      collapsible: false,
      items: ['intro', 'getting-started'],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsible: false,
      items: ['codex-to-claude', 'local-models', 'remote-control', 'usage'],
    },
    {
      type: 'category',
      label: 'Reference',
      collapsible: false,
      items: ['providers', 'agents', 'configuration', 'troubleshooting'],
    },
  ],
};

export default sidebars;
