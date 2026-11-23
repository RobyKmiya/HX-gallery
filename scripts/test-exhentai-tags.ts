/**
 * ExHentai 标签解析器自检。
 *
 * 跑法（Node 22 自带类型剥离，不需要装任何东西）：
 *   node --experimental-strip-types scripts/test-exhentai-tags.ts
 *
 * 覆盖：单标签 / 多标签 / 含空格标签 / 多分类 / 空白格 / 重复 / 无链接 /
 * 列数不一 / 空输入 / 残疾 Markdown / URL 里的 + 与 %28 / 同分类跨行 /
 * 大小写 / 无冒号分类 / 转义竖线 / 标签名带逗号 / 未知分类 / 超长 /
 * HTML 表格 / 幂等 / 全角符号，以及 tags.ts 的既有路径回归。
 */

import {
  decodeExhTagToken,
  markdownTablesToTagLines,
  parseExHentaiTags,
} from "../src/exhentaiTagParser.ts";
import { parseTagInput, tagsToCopyText } from "../src/tags.ts";

let passed = 0;
const failures: string[] = [];

function eq(name: string, actual: unknown, expected: unknown) {
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a === b) {
    passed += 1;
    return;
  }
  failures.push(`${name}\n    实际: ${a}\n    期望: ${b}`);
}

function ok(name: string, condition: boolean, detail = "") {
  if (condition) {
    passed += 1;
    return;
  }
  failures.push(`${name}${detail ? `\n    ${detail}` : ""}`);
}

const URL = (ns: string, name: string) =>
  `https://exhentai.org/tag/${ns}:${name.replace(/ /g, "+")}`;

// ---------------------------------------------------------------- 1 单标签
{
  const input = `| parody: | [**arknights**](${URL("parody", "arknights")}) |`;
  const r = parseExHentaiTags(input);
  eq("1 单标签 · plainText", r.plainText, "parody: arknights");
  eq("1 单标签 · 统计", [r.statistics.categoryCount, r.statistics.tagCount], [1, 1]);
  eq("1 单标签 · success", r.success, true);
}

// ------------------------------------------------------- 2 用户给的完整例子
const USER_INPUT = [
  `| parody: | [**arknights**](${URL("parody", "arknights")}) | [**genshin impact**](${URL("parody", "genshin impact")}) |\\`,
  `| female: | [**big breasts**](${URL("female", "big breasts")}) | [**blowjob**](${URL("female", "blowjob")}) |`,
].join("\n");
{
  const r = parseExHentaiTags(USER_INPUT);
  eq(
    "2 完整例子 · plainText",
    r.plainText,
    "parody: arknights，genshin impact\n\nfemale: big breasts，blowjob",
  );
  eq("2 完整例子 · 统计", [r.statistics.categoryCount, r.statistics.tagCount], [2, 4]);
  eq("2 完整例子 · 分类键顺序", Object.keys(r.categories), ["parody", "female"]);
}

// ------------------------------------------------------------ 3 含空格标签
{
  const r = parseExHentaiTags(USER_INPUT);
  eq("3 含空格 · genshin impact 完整", r.categories.parody, ["arknights", "genshin impact"]);
  eq("3 含空格 · big breasts 完整", r.categories.female, ["big breasts", "blowjob"]);
  ok(
    "3 含空格 · 标签数没被空格撑大",
    r.statistics.tagCount === 4,
    `tagCount=${r.statistics.tagCount}`,
  );
}
{
  const r = parseExHentaiTags("female: big breasts, sole female");
  eq("3b 无链接多词标签", r.categories.female, ["big breasts", "sole female"]);
}
{
  const r = parseExHentaiTags("female: multi-work series");
  eq("3c multi-work series 完整", r.categories.female, ["multi-work series"]);
}

// -------------------------------------------------------------- 4 多分类表格
{
  const input = [
    `| parody: | [**a**](${URL("parody", "a")}) |`,
    `| character: | [**b**](${URL("character", "b")}) |`,
    `| artist: | [**c**](${URL("artist", "c")}) |`,
    `| female: | [**d**](${URL("female", "d")}) |`,
    `| male: | [**e**](${URL("male", "e")}) |`,
  ].join("\n");
  const r = parseExHentaiTags(input);
  eq("4 多分类 · 键顺序", Object.keys(r.categories), [
    "parody",
    "character",
    "artist",
    "female",
    "male",
  ]);
  eq("4 多分类 · 分类数", r.statistics.categoryCount, 5);
  eq("4 多分类 · 标签数", r.statistics.tagCount, 5);
}

// -------------------------------------------------------------- 5 空白单元格
{
  const input = `| female: | [**x**](${URL("female", "x")}) |  | - |`;
  const r = parseExHentaiTags(input);
  eq("5 空白格 · 只收 1 个", r.categories.female, ["x"]);
  ok("5 空白格 · emptyCells 计数", r.statistics.emptyCells >= 2, `emptyCells=${r.statistics.emptyCells}`);
  ok(
    "5 空白格 · 没有空标签",
    r.blocks.every((block) => block.tags.every((tag) => tag.trim() !== "")),
  );
}

// ---------------------------------------------------------------- 6 重复标签
{
  const row = `| female: | [**big breasts**](${URL("female", "big breasts")}) |`;
  const r = parseExHentaiTags(`${row}\n${row}`);
  eq("6 重复 · 只留一个", r.categories.female, ["big breasts"]);
  eq("6 重复 · duplicatesRemoved", r.statistics.duplicatesRemoved, 1);
}
{
  const row = `| female: | [**big breasts**](${URL("female", "big breasts")}) |`;
  const r = parseExHentaiTags(`${row}\n${row}`, { dedupe: false });
  eq("6b 关掉去重 · 留两个", r.categories.female, ["big breasts", "big breasts"]);
}

// ------------------------------------------------------ 7 没有超链接的纯文本
{
  const r = parseExHentaiTags("| artist: | mountainhan |");
  eq("7 纯文本 · 单标签", r.plainText, "artist: mountainhan");
}
{
  const r = parseExHentaiTags("| female: | big breasts |");
  eq("7b 纯文本 · 空格不切", r.categories.female, ["big breasts"]);
}
{
  const r = parseExHentaiTags("| female: | big breasts, dark skin |");
  eq("7c 纯文本 · 逗号才切", r.categories.female, ["big breasts", "dark skin"]);
}

// ------------------------------------------------------------ 8 不同列数表格
{
  const input = [
    `| parody: | [**a**](${URL("parody", "a")}) |`,
    `| female: | [**b**](${URL("female", "b")}) | [**c**](${URL("female", "c")}) | [**d**](${URL("female", "d")}) |`,
    "| male: |",
  ].join("\n");
  const r = parseExHentaiTags(input);
  eq("8 列数不一 · parody", r.categories.parody, ["a"]);
  eq("8 列数不一 · female 三列", r.categories.female, ["b", "c", "d"]);
  ok("8 列数不一 · 单列行不出标签", r.categories.male === undefined);
  ok(
    "8 列数不一 · 单列行有提示",
    r.errors.some((error) => error.code === "EMPTY_ROW"),
  );
}

// ------------------------------------------------------------------ 9 空输入
{
  const r = parseExHentaiTags("");
  eq("9 空输入 · success", r.success, false);
  eq("9 空输入 · plainText", r.plainText, "");
  eq("9 空输入 · 错误码", r.errors[0]?.code, "EMPTY_INPUT");
}
{
  const r = parseExHentaiTags("   \n\n  ");
  eq("9b 全空白 · 错误码", r.errors[0]?.code, "EMPTY_INPUT");
}

// ------------------------------------------------------------ 10 残疾 Markdown
{
  const r = parseExHentaiTags(
    `| parody: | [**arknights**(${URL("parody", "arknights")} |`,
  );
  eq("10 缺右括号 · 从 URL 救回", r.categories.parody, ["arknights"]);
  ok("10 缺右括号 · 有提示", r.errors.some((error) => error.code === "UNCLOSED_BRACKET"));
}
{
  const r = parseExHentaiTags("|||");
  ok("10b 纯竖线 · 不抛异常", r.success === false && r.statistics.tagCount === 0);
}
{
  let threw = false;
  try {
    parseExHentaiTags("%%%\n|||\n[a](\n:::");
  } catch {
    threw = true;
  }
  ok("10c 乱码 · 不抛异常", !threw);
}

// --------------------------------------------------- 11 URL 里的 + 与 %28%29
{
  eq(
    "11 + 还原空格",
    decodeExhTagToken("female:big+breasts"),
    "female:big breasts",
  );
  eq(
    "11b %28%29 还原括号",
    decodeExhTagToken("character:kafka%28honkai%29"),
    "character:kafka(honkai)",
  );
  eq("11c 字面加号 %2B 不被当空格", decodeExhTagToken("other:a%2Bb"), "other:a+b");
}
{
  const r = parseExHentaiTags(
    `| female: | [**big breasts**](${URL("female", "big breasts")}) |`,
  );
  eq("11d URL 带 + · 标签完整", r.categories.female, ["big breasts"]);
}

// ------------------------------------------------------ 12 同一分类跨多行
{
  const input = [
    `| female: | [**a**](${URL("female", "a")}) |`,
    `| female: | [**b**](${URL("female", "b")}) |`,
  ].join("\n");
  const r = parseExHentaiTags(input);
  eq("12 跨行合并 · 分类数", r.statistics.categoryCount, 1);
  eq("12 跨行合并 · 保序", r.categories.female, ["a", "b"]);
}

// ------------------------------------------------------------ 13 分类大小写
{
  const r = parseExHentaiTags(`| Parody: | [**a**](${URL("parody", "a")}) |`);
  eq("13 分类大小写归一", Object.keys(r.categories), ["parody"]);
}

// -------------------------------------------------------------- 14 无冒号分类
{
  const r = parseExHentaiTags(`| parody | [**a**](${URL("parody", "a")}) |`);
  eq("14 无冒号分类", Object.keys(r.categories), ["parody"]);
}

// ---------------------------------------------------------------- 15 转义竖线
{
  const r = parseExHentaiTags(
    `| female: | [**a**](${URL("female", "a")}) \\| [**b**](${URL("female", "b")}) |`,
  );
  eq("15 转义竖线 · 仍是两个标签", r.categories.female, ["a", "b"]);
}

// ------------------------------------------------------- 16 标签名里带逗号
{
  const r = parseExHentaiTags(
    `| female: | [**a, b**](${URL("female", "a b")}) |`,
  );
  eq("16 链接文本优先 · 逗号不切", r.categories.female, ["a, b"]);
}

// ------------------------------------------------------------ 17 未知分类
{
  const r = parseExHentaiTags(`| location: | [**x**](${URL("location", "x")}) |`);
  eq("17 未知分类保留原键", Object.keys(r.categories), ["location"]);
}
{
  const r = parseExHentaiTags("| 中文分类： | [**y**](../y) |");
  ok(
    "17b 中文分类不被吞",
    Object.keys(r.categories).length === 1,
    JSON.stringify(Object.keys(r.categories)),
  );
}

// ---------------------------------------------------------------- 18 超长标签
{
  const long = "x".repeat(120);
  const r = parseExHentaiTags(`| female: | ${long} |`);
  eq("18 超长 · 不收", r.statistics.tagCount, 0);
  eq("18 超长 · 计数", r.statistics.overlong, 1);
  ok("18 超长 · 有提示", r.errors.some((error) => error.code === "OVERLONG_TAG"));
}

// ------------------------------------------------------------ 19 HTML 表格
{
  const input =
    '<table><tbody><tr><td class="tc">parody:</td>' +
    `<td><div><a href="${URL("parody", "arknights")}">arknights</a></div></td>` +
    `<td><div><a href="${URL("parody", "genshin impact")}">genshin impact</a></div></td>` +
    "</tr></tbody></table>";
  const r = parseExHentaiTags(input);
  eq("19 HTML 表格", r.plainText, "parody: arknights，genshin impact");
}

// ---------------------------------------------------------------- 20 幂等性
{
  const first = parseExHentaiTags(USER_INPUT).plainText;
  const second = parseExHentaiTags(first).plainText;
  eq("20 输出可再解析（幂等）", second, first);
}
{
  const r = parseExHentaiTags(USER_INPUT, {
    tagSeparator: ", ",
    blockSeparator: "\n",
  });
  eq("20b 英文逗号 + 不空行", r.plainText, "parody: arknights, genshin impact\nfemale: big breasts, blowjob");
}

// ------------------------------------------------------------ 21 全角符号
{
  const r = parseExHentaiTags(
    `｜ parody： ｜ [**a**](${URL("parody", "a")}) ｜`,
  );
  eq("21 全角竖线 + 全角冒号", r.plainText, "parody: a");
}

// ------------------------------------------------- 22 分类筛选 / 排除
{
  const r = parseExHentaiTags(USER_INPUT, { includeCategories: ["female"] });
  eq("22 只保留 female", Object.keys(r.categories), ["female"]);
  const r2 = parseExHentaiTags(USER_INPUT, { excludeCategories: ["parody"] });
  eq("22b 排除 parody", Object.keys(r2.categories), ["female"]);
}

// ------------------------------------------- 23 表格 → tags.ts 桥接行为
{
  const bridged = markdownTablesToTagLines(USER_INPUT);
  eq(
    "23 表格转行",
    bridged,
    "parody: arknights\tgenshin impact\nfemale: big breasts\tblowjob",
  );
  const r = parseTagInput(USER_INPUT);
  eq(
    "23b parseTagInput 吃到表格 · 标签名",
    r.tags.map((tag) => tag.name),
    ["arknights", "genshin impact", "big breasts", "blowjob"],
  );
  eq(
    "23c parseTagInput 吃到表格 · 大类",
    r.tags.map((tag) => tag.category),
    ["原作", "原作", "女性", "女性"],
  );
}

// ------------------------------------------- 24 tags.ts 既有路径回归
{
  const r = parseTagInput("female:big breasts, sole female\nartist:mountainhan");
  eq(
    "24 旧格式不变",
    r.tags.map((tag) => `${tag.category}/${tag.name}`),
    ["女性/big breasts", "女性/sole female", "作者/mountainhan"],
  );
}
{
  const r = parseTagInput("全彩, 纯爱；萝莉、黑白");
  eq("24b 裸标签不变", r.tags.map((tag) => tag.name), ["全彩", "纯爱", "萝莉", "黑白"]);
  eq("24c 裸标签无分类", r.tags.every((tag) => tag.category === null), true);
}
{
  const tags = [
    { name: "big breasts", category: "女性" },
    { name: "arknights", category: "原作" },
  ];
  const text = tagsToCopyText(tags);
  const back = parseTagInput(text);
  eq(
    "24d 复制 → 回贴 可逆",
    back.tags.map((tag) => `${tag.category}/${tag.name}`),
    ["女性/big breasts", "原作/arknights"],
  );
}
{
  // 没有竖线的输入必须逐字符不动，保证既有粘贴手感不变
  const plain = "female:big breasts\n随机一段话 里面 有空格";
  eq("24e 无竖线输入不被改写", markdownTablesToTagLines(plain), plain);
}

// ---------------------------------------------- 25 网页复制的制表符分隔
{
  const r = parseExHentaiTags("parody:\tarknights\tgenshin impact");
  eq("25 制表符当列边界", r.categories.parody, ["arknights", "genshin impact"]);
}
{
  const r = parseExHentaiTags("parody:\narknights\ngenshin impact\nfemale:\nbig breasts");
  eq("25b 一类一行 + 标签各一行", Object.keys(r.categories), ["parody", "female"]);
  eq("25c 继承分类", r.categories.parody, ["arknights", "genshin impact"]);
  eq("25d 换类后归新类", r.categories.female, ["big breasts"]);
}

// ---------------------------------- 26 下游不认识的分类不拼成垃圾标签
{
  eq(
    "26 桥接 · 未知分类不带头",
    markdownTablesToTagLines("| location: | x |", ["parody", "female"]),
    "x",
  );
  eq(
    "26b 桥接 · 已知分类带头",
    markdownTablesToTagLines("| parody: | x |", ["parody"]),
    "parody: x",
  );
  const r = parseTagInput("| location: | x |");
  eq("26c 未知分类 → 裸标签", r.tags.map((tag) => tag.name), ["x"]);
  eq("26d 未知分类 → 无大类", r.tags[0]?.category, null);
}

// ------------------------------------------- 27 纯文本 / HTML 粘贴（无表格竖线）
{
  // 网页复制成纯文本、或浏览器扩展只给链接不给表格：一行一类，标签横排
  const input = [
    `parody: [**arknights**](${URL("parody", "arknights")}) [**genshin impact**](${URL("parody", "genshin impact")})`,
    `female: [**big breasts**](${URL("female", "big breasts")}) [**blowjob**](${URL("female", "blowjob")})`,
  ].join("\n");
  const r = parseExHentaiTags(input);
  eq(
    "27a 纯文本链接横排 · plainText",
    r.plainText,
    "parody: arknights，genshin impact\n\nfemale: big breasts，blowjob",
  );
  eq("27b 纯文本链接横排 · 标签数", r.statistics.tagCount, 4);

  // 链接竖排：后续行没有分类头，继承上一行
  const stacked = [
    `parody: [**arknights**](${URL("parody", "arknights")})`,
    `[**genshin impact**](${URL("parody", "genshin impact")})`,
    `female: [**big breasts**](${URL("female", "big breasts")})`,
  ].join("\n");
  eq(
    "27c 纯文本链接竖排 · plainText",
    parseExHentaiTags(stacked).plainText,
    "parody: arknights，genshin impact\n\nfemale: big breasts",
  );

  // 裸 URL 堆着（项目原有能力，这里锁住不被新逻辑改坏）
  const bareUrl = [
    `parody: ${URL("parody", "arknights")} ${URL("parody", "genshin impact")}`,
  ].join("\n");
  eq(
    "27d 光秃 URL · plainText",
    parseExHentaiTags(bareUrl).plainText,
    "parody: arknights，genshin impact",
  );

  // HTML 片段：<a> 必须先转 markdown 再丢标签，否则空格一丢就分不开了
  const htmlDiv =
    `<div>parody: <a href="${URL("parody", "arknights")}">arknights</a>` +
    `<a href="${URL("parody", "genshin impact")}">genshin impact</a></div>`;
  eq(
    "27e HTML div 片段 · plainText",
    parseExHentaiTags(htmlDiv).plainText,
    "parody: arknights，genshin impact",
  );

  const htmlTable =
    `<table><tr><td>female:</td><td><a href="${URL("female", "big breasts")}">big breasts</a></td>` +
    `<td><a href="${URL("female", "blowjob")}">blowjob</a></td></tr></table>`;
  eq(
    "27f HTML 表格 · plainText",
    parseExHentaiTags(htmlTable).plainText,
    "female: big breasts，blowjob",
  );

  // 制表符分隔（表格复制到记事本再粘回来）
  const tabbed = ["parody:\tarknights\tgenshin impact", "female:\tbig breasts\tblowjob"].join("\n");
  eq(
    "27g 制表符分隔 · plainText",
    parseExHentaiTags(tabbed).plainText,
    "parody: arknights，genshin impact\n\nfemale: big breasts，blowjob",
  );

  // 最惨的情况：纯空格，没有任何边界 —— 不硬拆，但必须给一条明确提示
  const glued = parseExHentaiTags("arknights genshin impact big breasts blowjob");
  eq("27h 纯空格粘连 · 不硬拆", glued.statistics.tagCount, 1);
  ok(
    "27i 纯空格粘连 · 有 SUSPICIOUS_LONG_TAG 提示",
    glued.errors.some((error) => error.code === "SUSPICIOUS_LONG_TAG"),
  );

  // 批量导入（parseTagInput）这条也要能吃无竖线的链接行
  eq(
    "27j parseTagInput · 无竖线链接行",
    parseTagInput(input).tags.map((tag) => `${tag.category}/${tag.name}`),
    ["原作/arknights", "原作/genshin impact", "女性/big breasts", "女性/blowjob"],
  );
  eq(
    "27k parseTagInput · HTML 片段",
    parseTagInput(htmlDiv).tags.map((tag) => tag.name),
    ["arknights", "genshin impact"],
  );

  // 保护性：既没竖线也没链接的纯文本，必须逐字不变（不能碰既有行为）
  const plainComma = "female: big breasts, sole female";
  eq("27l 无链接纯文本 · 桥接原样返回", markdownTablesToTagLines(plainComma), plainComma);
}

// ------------------------------------------------------------------ 汇总
if (failures.length === 0) {
  console.log(`全部通过：${passed} 项`);
} else {
  console.log(`通过 ${passed} 项，失败 ${failures.length} 项：\n`);
  for (const failure of failures) console.log(`  ✗ ${failure}\n`);
  process.exitCode = 1;
}
