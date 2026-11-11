import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";
import de from "./locales/de.json";
import fr from "./locales/fr.json";
import zh from "./locales/zh.json";

export const LANGUAGES: { code: string; label: string }[] = [
  { code: "en", label: "English" },
  { code: "de", label: "Deutsch" },
  { code: "fr", label: "Français" },
  { code: "zh", label: "中文" },
];

export function detectLanguage(): string {
  const wanted = navigator.language.slice(0, 2).toLowerCase();
  return LANGUAGES.some((l) => l.code === wanted) ? wanted : "en";
}

void i18n.use(initReactI18next).init({
  resources: { en: { translation: en }, de: { translation: de }, fr: { translation: fr }, zh: { translation: zh } },
  lng: detectLanguage(),
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

export default i18n;