import { describe, expect, it } from 'vitest';
import { normalizeAnthropicUrl, normalizeOpenaiUrl } from './normalizeUrl';

describe('normalizeOpenaiUrl', () => {
  // The motivating case: a provider's copy button yields the full endpoint.
  it('strips a trailing /v1/chat/completions down to /v1', () => {
    expect(normalizeOpenaiUrl('https://api.openai.com/v1/chat/completions')).toBe(
      'https://api.openai.com/v1'
    );
  });

  it('tolerates a trailing slash on the endpoint', () => {
    expect(normalizeOpenaiUrl('https://api.openai.com/v1/chat/completions/')).toBe(
      'https://api.openai.com/v1'
    );
  });

  it('leaves a clean /v1 base untouched', () => {
    expect(normalizeOpenaiUrl('https://api.openai.com/v1')).toBe('https://api.openai.com/v1');
  });

  it('leaves a bare host untouched', () => {
    expect(normalizeOpenaiUrl('https://api.openai.com')).toBe('https://api.openai.com');
  });

  it('strips a bare /chat/completions tail when the base has no /v1', () => {
    expect(normalizeOpenaiUrl('https://gateway.example.com/chat/completions')).toBe(
      'https://gateway.example.com'
    );
  });

  it('is case-insensitive', () => {
    expect(normalizeOpenaiUrl('https://x/v1/Chat/Completions')).toBe('https://x/v1');
  });
});

describe('normalizeAnthropicUrl', () => {
  it('strips a trailing /v1/messages down to the bare host', () => {
    expect(normalizeAnthropicUrl('https://api.anthropic.com/v1/messages')).toBe(
      'https://api.anthropic.com'
    );
  });

  it('strips a bare /messages endpoint with no /v1', () => {
    expect(normalizeAnthropicUrl('https://api.anthropic.com/messages')).toBe(
      'https://api.anthropic.com'
    );
  });

  it('tolerates a trailing slash', () => {
    expect(normalizeAnthropicUrl('https://api.anthropic.com/v1/messages/')).toBe(
      'https://api.anthropic.com'
    );
  });

  it('leaves a bare host untouched', () => {
    expect(normalizeAnthropicUrl('https://api.anthropic.com')).toBe('https://api.anthropic.com');
  });
});
