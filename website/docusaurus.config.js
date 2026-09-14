// @ts-check
import {themes as prismThemes} from 'prism-react-renderer';

const tagline =
  'Claude Code on your ChatGPT plan — and seven other coding agents on any ' +
  'provider, from one command.';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'all-code (alc)',
  tagline,
  favicon: 'img/favicon.png',

  url: 'https://treeleaves30760.github.io',
  baseUrl: '/all-code/',
  organizationName: 'treeleaves30760',
  projectName: 'all-code',
  trailingSlash: false,

  // @docusaurus/faster is already a dependency (package.json); `faster: true`
  // turns on rspack, swc, lightningcss and worker-thread SSG. `v4: true`
  // puts Infima and the theme CSS in cascade layers, so custom.css wins
  // without specificity games.
  future: {
    v4: true,
    faster: true,
  },

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en', 'zh-TW'],
    localeConfigs: {
      en: {label: 'English', htmlLang: 'en-US'},
      'zh-TW': {label: '繁體中文', htmlLang: 'zh-TW'},
    },
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          sidebarPath: './sidebars.js',
          routeBasePath: '/',
          editUrl:
            'https://github.com/treeleaves30760/all-code/tree/main/website/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
        sitemap: {
          changefreq: 'weekly',
          priority: 0.5,
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      image: 'img/og.png',
      metadata: [
        {name: 'description', content: tagline},
        {
          name: 'keywords',
          content:
            'claude code, codex cli, chatgpt subscription, opencode, pi coding agent, ' +
            'copilot cli, goose, qwen code, kimi code cli, llm provider, anthropic, ' +
            'openai, openrouter, ollama, vllm, usage, quota, remote control, coding agent, cli, rust',
        },
        {property: 'og:type', content: 'website'},
        {name: 'twitter:card', content: 'summary_large_image'},
      ],
      colorMode: {
        defaultMode: 'dark',
        respectPrefersColorScheme: true,
      },
      docs: {
        sidebar: {
          hideable: false,
          autoCollapseCategories: false,
        },
      },
      tableOfContents: {
        minHeadingLevel: 2,
        maxHeadingLevel: 3,
      },
      navbar: {
        title: 'all-code',
        hideOnScroll: false,
        logo: {alt: 'all-code logo', src: 'img/logo.svg', width: 28, height: 28},
        items: [
          {
            type: 'docSidebar',
            sidebarId: 'docs',
            position: 'left',
            label: 'Docs',
          },
          {type: 'localeDropdown', position: 'right'},
          {
            href: 'https://github.com/treeleaves30760/all-code/releases/latest',
            label: 'Download',
            position: 'right',
          },
          {
            href: 'https://github.com/treeleaves30760/all-code',
            label: 'GitHub',
            position: 'right',
          },
        ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              {label: 'Overview', to: '/'},
              {label: 'Getting started', to: '/getting-started'},
              {label: 'Codex bridge', to: '/codex-to-claude'},
              {label: 'Usage', to: '/usage'},
              {label: 'Troubleshooting', to: '/troubleshooting'},
            ],
          },
          {
            title: 'Project',
            items: [
              {
                label: 'GitHub',
                href: 'https://github.com/treeleaves30760/all-code',
              },
              {
                label: 'Releases',
                href: 'https://github.com/treeleaves30760/all-code/releases',
              },
              {
                label: 'Issues',
                href: 'https://github.com/treeleaves30760/all-code/issues',
              },
            ],
          },
          {
            title: 'Agents',
            items: [
              {
                label: 'Claude Code',
                href: 'https://code.claude.com/docs/en/setup',
              },
              {
                label: 'Codex CLI',
                href: 'https://learn.chatgpt.com/docs/codex/cli',
              },
              {label: 'OpenCode', href: 'https://opencode.ai/docs'},
              {label: 'Pi', href: 'https://github.com/earendil-works/pi'},
              {
                label: 'Copilot CLI',
                href: 'https://docs.github.com/en/copilot/how-tos/copilot-cli',
              },
              {label: 'Goose', href: 'https://block.github.io/goose/'},
              {
                label: 'Qwen Code',
                href: 'https://github.com/QwenLM/qwen-code',
              },
              {
                label: 'Kimi Code CLI',
                href: 'https://github.com/MoonshotAI/kimi-cli',
              },
            ],
          },
        ],
        copyright: 'MIT licensed · treeleaves30760',
      },
      prism: {
        theme: prismThemes.oneLight,
        darkTheme: prismThemes.oneDark,
        additionalLanguages: ['bash', 'powershell', 'toml', 'json'],
      },
    }),
};

export default config;
