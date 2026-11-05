// The whole frames in what the stream has sent so far, and the partial one
// that waits for the rest.
export function frames(text: string): { whole: string[]; rest: string } {
  const whole = text.split('\n\n');
  const rest = whole.pop() ?? '';
  return { whole, rest };
}

export const isChange = (frame: string) => frame.split('\n').includes('data: changed');
