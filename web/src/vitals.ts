import { ago, type Tone } from './format';
import type { Level, Vital } from './types';

const order: Level[] = ['ok', 'info', 'warn', 'bad'];

export function worst(vitals: Vital[]): Level {
  return vitals.reduce<Level>((w, v) => (order.indexOf(v.level) > order.indexOf(w) ? v.level : w), 'ok');
}

export function tone(level: Level): Tone {
  return level === 'bad' || level === 'warn' ? level : '';
}

export function detail(v: Vital, now: number): string {
  return v.since === undefined ? v.detail : v.detail.replace('{since}', ago(v.since, now));
}

export type Part = { kind: 'text' | 'code' | 'link'; text: string };

export function parts(text: string): Part[] {
  return text.split('`').flatMap((chunk, i): Part[] =>
    i % 2
      ? [{ kind: 'code', text: chunk }]
      : chunk.split(/(https:\/\/[^\s)]+)/).map((t, j) => ({ kind: j % 2 ? 'link' : 'text', text: t })),
  ).filter((p) => p.text);
}
