import { describe, expect, it } from 'vitest';
import { frames, isChange } from './sse';

describe('frames', () => {
  it('keeps a partial frame for the next chunk', () => {
    expect(frames('data: changed\n\ndata: cha')).toEqual({ whole: ['data: changed'], rest: 'data: cha' });
    expect(frames('data: cha' + 'nged\n\n')).toEqual({ whole: ['data: changed'], rest: '' });
  });

  it('splits several frames sent at once', () => {
    expect(frames(': ping\n\ndata: changed\n\n').whole).toEqual([': ping', 'data: changed']);
  });

  it('has nothing whole before a blank line arrives', () => {
    expect(frames('')).toEqual({ whole: [], rest: '' });
  });
});

describe('isChange', () => {
  it('takes a frame whose data is changed, among its other lines', () => {
    expect(isChange('data: changed')).toBe(true);
    expect(isChange('event: x\ndata: changed')).toBe(true);
  });

  it('ignores comments and other data', () => {
    expect(isChange(': ping')).toBe(false);
    expect(isChange('data: changed later')).toBe(false);
  });
});
