/** @vitest-environment jsdom */

import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Markdown } from './Markdown.tsx';

describe('Markdown', () => {
  it('keeps inline code inline and block code outside paragraphs', () => {
    const { container } = render(
      <Markdown>
        {'Use `pnpm test` here.\n\n```ts\nconst ok = true;\n```'}
      </Markdown>,
    );

    expect(container.querySelector('p code')?.textContent).toBe('pnpm test');
    expect(container.querySelector('p div')).toBeNull();
    expect(container.querySelector('p pre')).toBeNull();
    expect(container.querySelector('pre code')?.textContent).toContain(
      'const ok = true;',
    );
  });

  it('highlights fenced code with a supported language', () => {
    const { container } = render(
      <Markdown>
        {'```typescript\nconst answer: number = 42;\nconsole.log(answer);\n```'}
      </Markdown>,
    );

    const code = container.querySelector(
      'code[data-highlight-language="typescript"]',
    );
    expect(code?.textContent).toContain('const answer: number = 42;');
    expect(code?.innerHTML).toContain('hljs-keyword');
    expect(code?.innerHTML).toContain('hljs-number');
  });

  it('renders unsupported languages as escaped plain text', () => {
    const { container } = render(
      <Markdown>
        {'```made-up-language\n<script>alert("safe")</script>\n```'}
      </Markdown>,
    );

    const code = container.querySelector(
      'code[data-highlight-language="made-up-language"]',
    );
    expect(code?.textContent).toBe('<script>alert("safe")</script>');
    expect(code?.querySelector('script')).toBeNull();
  });

  it('escapes source markup before inserting highlighted token spans', () => {
    const { container } = render(
      <Markdown>{'```html\n<img src="x" onerror="alert(1)">\n```'}</Markdown>,
    );

    const code = container.querySelector(
      'code[data-highlight-language="html"]',
    );
    expect(code?.textContent).toBe('<img src="x" onerror="alert(1)">');
    expect(code?.querySelector('img')).toBeNull();
    expect(code?.innerHTML).toContain('hljs-tag');
  });

  it('renders GFM tables with the scrollable table class', () => {
    const { container } = render(
      <Markdown>
        {
          '| Name | Status |\n| --- | ---: |\n| formatter | passing |\n| tests | 42 |'
        }
      </Markdown>,
    );

    const table = container.querySelector('table');
    expect(table?.className).not.toBe('');
    expect(
      Array.from(table?.querySelectorAll('th') ?? []).map(
        (cell) => cell.textContent,
      ),
    ).toEqual(['Name', 'Status']);
    expect(table?.querySelectorAll('tbody tr')).toHaveLength(2);
    expect(table?.closest('p')).toBeNull();
  });
});
