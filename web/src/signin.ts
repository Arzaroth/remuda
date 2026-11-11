// An answer a sign-in asks for beside the key (`provider::Field`): one of
// `choices`, the first by default, or free text that may be left out.
export type Field = { name: string; label: string; choices: string[] };

// What a sign-in starts with: every choice on its default, every text empty.
export const defaults = (fields: Field[]): Record<string, string> =>
  Object.fromEntries(fields.map((f) => [f.name, f.choices[0] ?? '']));

// What `/api/login/finish` is sent: only the fields the sign-in asked for,
// trimmed, a choice left blank back on its default.
export function answers(fields: Field[], typed: Record<string, string>): Record<string, string> {
  return Object.fromEntries(
    fields.map((f) => {
      const value = (typed[f.name] ?? '').trim();
      return [f.name, value || (f.choices[0] ?? '')];
    }),
  );
}

// The words a field is shown under, from a label that reads inside a sentence.
export const heading = (f: Field): string => {
  const words = f.label.replace(/^the /, '').split(',')[0];
  return words.charAt(0).toUpperCase() + words.slice(1);
};
