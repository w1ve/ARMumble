import { describe, expect, it } from "vitest";
import de from "./locales/de.json";
import en from "./locales/en.json";
import fr from "./locales/fr.json";
import zh from "./locales/zh.json";

function keys(obj: Record<string, unknown>, prefix = ""): string[] {
  return Object.entries(obj).flatMap(([k, v]) =>
    typeof v === "object" && v !== null ? keys(v as Record<string, unknown>, `${prefix}${k}.`) : [`${prefix}${k}`],
  );
}

describe("locales", () => {
  const reference = keys(en).sort();
  for (const [name, locale] of Object.entries({ de, fr, zh })) {
    it(`${name} has exactly the English keys`, () => {
      expect(keys(locale).sort()).toEqual(reference);
    });
    it(`${name} has no empty strings`, () => {
      expect(keys(locale).filter((k) => k.length === 0)).toEqual([]);
    });
  }
});
