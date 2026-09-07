// Checks every locale file against English, which is the reference.
//
//   node tools/locales/check.js
//
// A translation is not just a set of sentences: a missing key shows the key
// itself on screen, and a placeholder dropped or renamed leaves a hole where a
// filename or a count should be. Both are easy to do by hand and invisible
// until somebody hits that screen, so they are checked here instead.

const fs = require("fs");
const path = require("path");

const DIR = path.resolve(__dirname, "../../ui/locales");
const REFERENCE = "en";

const placeholders = (text) =>
  new Set([...String(text).matchAll(/\{(\w+)\}/g)].map((m) => m[1]));

const files = fs.readdirSync(DIR).filter((f) => f.endsWith(".json"));
const load = (tag) => JSON.parse(fs.readFileSync(path.join(DIR, `${tag}.json`), "utf8"));

const reference = load(REFERENCE);
const problems = [];

for (const file of files) {
  const tag = file.replace(/\.json$/, "");
  if (tag === REFERENCE) continue;
  const strings = load(tag);

  for (const key of Object.keys(reference)) {
    if (!(key in strings)) problems.push(`${tag}: falta ${key}`);
  }
  for (const key of Object.keys(strings)) {
    if (!(key in reference)) problems.push(`${tag}: ${key} não existe em ${REFERENCE}`);
  }

  for (const [key, text] of Object.entries(strings)) {
    if (!(key in reference)) continue;
    const wanted = placeholders(reference[key]);
    const got = placeholders(text);
    for (const name of wanted) {
      if (!got.has(name)) problems.push(`${tag}: ${key} perdeu {${name}}`);
    }
    for (const name of got) {
      if (!wanted.has(name)) problems.push(`${tag}: ${key} inventou {${name}}`);
    }
    if (String(text).trim() === "") problems.push(`${tag}: ${key} está vazia`);
  }
}

// Counted messages need both forms, even in languages that do not inflect —
// the code asks for one of the two and would otherwise print the key.
for (const file of files) {
  const tag = file.replace(/\.json$/, "");
  const strings = load(tag);
  for (const key of Object.keys(strings)) {
    const base = key.replace(/\.(one|other)$/, "");
    if (base === key) continue;
    for (const form of ["one", "other"]) {
      if (!(`${base}.${form}` in strings)) problems.push(`${tag}: falta ${base}.${form}`);
    }
  }
}

if (problems.length) {
  console.error([...new Set(problems)].sort().join("\n"));
  console.error(`\n${new Set(problems).size} problema(s) nas traduções`);
  process.exit(1);
}
console.log(`${files.length} idiomas, ${Object.keys(reference).length} chaves, tudo alinhado.`);
