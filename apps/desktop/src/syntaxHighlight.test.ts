import { expect, it } from "vitest";
import { common } from "lowlight";
import { languageForPath } from "./syntaxHighlight";

it("recognizes a file for every loaded grammar", () => {
  const paths = ['sketch.ino', 'run.sh', 'main.c', 'main.cpp', 'main.cs', 'style.css', 'change.patch', 'main.go', 'query.gql', 'config.ini', 'Main.java', 'app.js', 'data.json', 'main.kt', 'style.less', 'main.lua', 'Makefile', 'README.md', 'main.mm', 'main.pl', 'index.php', 'index.phtml', 'notes.txt', 'main.py', 'session.pycon', 'main.r', 'main.rb', 'main.rs', 'style.scss', 'session.shellsession', 'query.sql', 'main.swift', 'main.ts', 'main.vb', 'main.wat', 'index.html', 'config.yaml'];
  expect(new Set(paths.map(languageForPath))).toEqual(new Set(Object.keys(common)));
});
it.each([
  ['src/APP.TSX', 'typescript'], ['C:\\src\\MAIN.PHP', 'php'],
  ['src/Makefile', 'makefile'], ['build/Makefile.am', 'makefile'],
  ['Gemfile', 'ruby'], ['home/.zshrc', 'bash'], ['home/.Rprofile', 'r'],
  ['app.mjs', 'javascript'], ['app.cts', 'typescript'],
  ['folder.php/unknown', undefined], ['unknown.xyz', undefined], ['__proto__', undefined],
])("recognizes %s", (path, language) => {
  expect(languageForPath(path)).toBe(language);
});
