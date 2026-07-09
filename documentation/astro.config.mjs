import mdx from '@astrojs/mdx';
import starlight from '@astrojs/starlight';
import mermaid from 'astro-mermaid';
import { defineConfig } from 'astro/config';

export default defineConfig({
  integrations: [
    mermaid(),
    starlight({
      title: 'MiSTer MagiK',
      description: 'User and developer documentation for MiSTer MagiK.',
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/nigelb/mister-slint',
        },
      ],
      sidebar: [
        {
          label: 'Start Here',
          items: [''],
        },
        {
          label: 'User Guide',
          items: [
            'user-guide/getting-started',
            'user-guide/home',
            'user-guide/arcade-browsing',
            'user-guide/search',
            'user-guide/settings',
            'user-guide/controllers',
            'user-guide/library-and-media',
            'user-guide/launching-games',
            'user-guide/dialogs-and-recovery',
          ],
        },
        {
          label: 'Architecture',
          items: [
            'architecture/boot-and-process',
            'architecture/framebuffer-and-hdmi',
            'architecture/launcher-lifecycle',
            'architecture/composition',
            'architecture/catalog-and-preview',
            'architecture/agent-and-capture',
            'architecture/input-and-controllers',
            'architecture/benchmarking-and-safety',
          ],
        },
        {
          label: 'Contributing',
          items: [
            'contributing/workflow',
            'contributing/device-rules',
            'contributing/naming-and-knowledge',
          ],
        },
      ],
      customCss: ['./src/styles/custom.css'],
    }),
    mdx(),
  ],
});
