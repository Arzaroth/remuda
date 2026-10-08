import { createRoot } from 'solid-js';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createConfirm } from './confirm';

describe('createConfirm', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('confirms on a second click after 400 ms, within 4 s', () =>
    createRoot((dispose) => {
      const c = createConfirm();
      expect(c.ask('Remove?')).toBe(false);
      expect(c.text()).toBe('Remove?');
      vi.advanceTimersByTime(100);
      expect(c.ask('Remove?')).toBe(false);
      vi.advanceTimersByTime(400);
      expect(c.ask('Remove?')).toBe(true);
      expect(c.text()).toBeNull();
      dispose();
    }));

  it('disarms after 4 s', () =>
    createRoot((dispose) => {
      const c = createConfirm();
      c.ask('Remove?');
      vi.advanceTimersByTime(4000);
      expect(c.text()).toBeNull();
      expect(c.ask('Remove?')).toBe(false);
      dispose();
    }));

  it('asks again when it is about something else', () =>
    createRoot((dispose) => {
      const c = createConfirm();
      c.ask('Overwrite a?', 'claude/a');
      vi.advanceTimersByTime(1000);
      expect(c.ask('Overwrite b?', 'claude/b')).toBe(false);
      expect(c.text()).toBe('Overwrite b?');
      dispose();
    }));
});
