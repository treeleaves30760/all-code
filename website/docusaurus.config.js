// @ts-check
import {themes as prismThemes} from 'prism-react-renderer';

const tagline =
  'Configure your LLM providers once, then launch any of eight coding agents ' +
  'with any provider — including Claude Code on your Codex/ChatGPT login.';

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

  onBrokenLinks: 'throw',
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
            'claude code, codex cli, opencode, pi coding agent, copilot cli, ' +
            'goose, qwen code, kimi code cli, llm provider, anthropic, ' +
            'openai, openrouter, ollama, vllm, coding agent, cli, rust',
        },
        {property: 'og:type', content: 'website'},
        {name: 'twitter:card', content: 'summary_large_image'},
      ],
      colorMode: {
        defaultMode: 'dark',
        respectPrefersColorScheme: true,
      },
      navbar: {
        title: 'all-code',
        logo: {alt: 'all-code logo', src: 'img/logo.svg'},
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
              {label: 'Introduction', to: '/'},
              {label: 'Install', to: '/installation'},
              {label: 'Codex bridge', to: '/codex-to-claude'},
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
        copyright: `MIT licensed. Copyright © ${new Date().getFullYear()} treeleaves30760.`,
      },
      prism: {
        theme: prismThemes.github,
        darkTheme: prismThemes.dracula,
        additionalLanguages: ['bash', 'powershell', 'toml', 'json'],
      },
    }),
};

export default config;
