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
});
