// Re-apply the drop-zone wording in every locale.
//
// NOTE: keep this file UTF-8. Writing it through PowerShell's `ascii` encoding
// silently replaces every non-ASCII character with `?`, which then lands in the
// locale JSON files (that is exactly the "??????" bug this script fixes).
import fs from "node:fs";
import path from "node:path";

const strings = {
  en: {
    aria: "Drop audio files or folders here, or click to select",
    prompt: "Drop files or folders",
    supports: "Most formats, folders run as a batch",
  },
  zh: {
    aria: "将音频文件或文件夹拖放到此处，或点击选择",
    prompt: "拖放文件或文件夹",
    supports: "支持大多数格式，文件夹自动批量",
  },
  de: {
    aria: "Audiodateien oder Ordner hier ablegen oder klicken",
    prompt: "Dateien oder Ordner ablegen",
    supports: "Meiste Formate, Ordner automatisch",
  },
  es: {
    aria: "Suelta archivos o carpetas de audio aquí, o haz clic",
    prompt: "Suelta archivos o carpetas",
    supports: "Casi todo formato, carpetas en lote",
  },
  fr: {
    aria: "Déposez des fichiers ou dossiers audio ici, ou cliquez",
    prompt: "Déposez fichiers ou dossiers",
    supports: "La plupart des formats, dossiers en lot",
  },
  ja: {
    aria: "音声ファイルやフォルダをここにドロップ、またはクリック",
    prompt: "ファイルやフォルダをドロップ",
    supports: "ほとんどの形式、フォルダは一括処理",
  },
  ko: {
    aria: "오디오 파일이나 폴더를 여기에 놓거나 클릭하세요",
    prompt: "파일이나 폴더를 놓기",
    supports: "대부분 형식, 폴더는 일괄 처리",
  },
  ru: {
    aria: "Перетащите аудиофайлы или папки сюда либо нажмите",
    prompt: "Перетащите файлы или папки",
    supports: "Почти все форматы, папки пакетно",
  },
};

const localesDir = path.join("src", "i18n", "locales");

for (const [lang, fileDrop] of Object.entries(strings)) {
  const file = path.join(localesDir, lang, "translation.json");
  const json = JSON.parse(fs.readFileSync(file, "utf8"));
  json.actionBar.fileDrop = { ...json.actionBar.fileDrop, ...fileDrop };
  fs.writeFileSync(file, `${JSON.stringify(json, null, 2)}\n`, "utf8");
  console.log(`${lang}: ${json.actionBar.fileDrop.prompt}`);
}
