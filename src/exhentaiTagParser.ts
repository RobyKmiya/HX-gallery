/**
 * ExHentai 标签表格解析器。
 *
 * 输入是从 ExHentai 作品页复制出来的 Markdown 标签表格（或它的 HTML / 纯文本变体），
 * 输出「分类 → 标签数组」+ 可直接复制的纯文本。
 *
 * 三条硬规矩（这是整个模块的设计前提）：
 *
 * 1. **绝不按空格切分。** `big breasts`、`genshin impact`、`multi-work series`
 *    都是一个完整标签。切分边界只有两种：Markdown 表格单元格（`|`）、行内链接
 *    边界（`[...](...)`）。空格只用来压缩多余空白，不用来断词。
 * 2. **链接显示文本优先，URL 只做兜底与校验。** 显示文本是人能读的那个名字；
 *    URL 里的 `+` 只是空格的编码，用来反推比猜要可靠，所以只有当链接文本为空
 *    或只剩数字/标点时，才退回 URL。
 * 3. **无副作用。** 不改入参、不碰 DOM、不读全局状态、不发请求。同一个输入
 *    永远得到同一个输出，方便单测，也方便「原始解析 → 人工编辑 → 再解析」分层。
 *
 * 与 `src/tags.ts` 的分工：本模块负责「表格 → 结构化标签」，不碰「命名空间 →
 * 应用大类」的映射（那是 tags.ts 的 NAMESPACE_CATEGORIES），也不碰数据库。
 * 注意本文件**不 import 任何项目内模块** —— tags.ts 反过来 import 本模块做表格
 * 预处理，保持单向依赖，不会形成循环。
 */

/** 与 tags.ts:27 / 后端 MAX_TAG_NAME 对齐。超长标签拒收而不是截断。 */
export const MAX_TAG_NAME_LENGTH = 100;

/** 错误码。全部是「提示级」，解析器本身永不抛异常。 */
export type ParseErrorCode =
  | "EMPTY_INPUT"
  | "NO_TAGS"
  | "UNCLOSED_BRACKET"
  | "ORPHAN_ROW"
  | "EMPTY_ROW"
  | "OVERLONG_TAG"
  | "BAD_URL";

export interface ParseError {
  code: ParseErrorCode;
  message: string;
  /** 1 基行号，方便在输入框里定位。 */
  line?: number;
  /** 出问题的原文片段（截断过）。 */
  sample?: string;
}

export interface ParseStatistics {
  categoryCount: number;
  tagCount: number;
  /** 识别到的表格行数（不含空行、分隔行）。 */
  rowCount: number;
  /** 被去重丢掉的条数。 */
  duplicatesRemoved: number;
  /** 空白单元格个数。 */
  emptyCells: number;
  /** 超长被拒的个数。 */
  overlong: number;
}

export interface ParsedBlock {
  /** 分类键，如 `parody`；无法识别分类的行归到 `未分类`。 */
  key: string;
  /** 展示用标签（目前与 key 相同，留出翻译余地）。 */
  label: string;
  tags: string[];
}

export interface ParseResult {
  success: boolean;
  /** 保序数组形式，UI 直接遍历。 */
  blocks: ParsedBlock[];
  /** 与 blocks 同序的字典形式，方便按 key 取值。 */
  categories: Record<string, string[]>;
  plainText: string;
  statistics: ParseStatistics;
  errors: ParseError[];
}

export interface ParseOptions {
  /** 同分类去重，默认 true。 */
  dedupe?: boolean;
  /** 去重范围，默认 `category`；`global` 表示跨分类也去重（先到先得）。 */
  dedupeScope?: "category" | "global";
  /** 标签之间用什么连接，默认全角逗号 `，`。 */
  tagSeparator?: string;
  /** 分类块之间用什么分隔，默认一个空行。 */
  blockSeparator?: string;
  /** 只保留这些分类（不传 = 全部保留）。 */
  includeCategories?: string[];
  /** 丢掉这些分类。 */
  excludeCategories?: string[];
  /** 分类键是否统一小写，默认 true（`Parody` → `parody`）。 */
  normalizeCategoryCase?: boolean;
  /** 无法识别分类的行归到哪个键，默认 `未分类`。 */
  fallbackCategory?: string;
}

/** 裸词（没写冒号）也要认成分类的白名单。只用于「这格是分类还是标签」的判断，
 *  不影响未知分类 —— 带冒号的 `location:` 之类照样原样保留。 */
const KNOWN_CATEGORY_KEYS = new Set([
  "language", "lang",
  "parody", "copyright", "series", "work", "source",
  "character", "characters", "char",
  "artist", "artists", "authors", "author", "group", "circle", "studio",
  "female", "females", "girl",
  "male", "males", "boy",
  "mixed",
  "other", "misc", "meta", "cosplay", "reclass", "temp", "species", "general", "lore",
  "location", "locations",
]);

/** 空白单元格的等价写法。`\|` 的行尾延续符、纯竖线也算空。 */
const FILLER_CELLS = new Set([
  "", "-", "--", "—", "–", "…", "...", "n/a", "na", "none", "null", "无", "空",
  "&nbsp;", "\u00a0", "\\", "|", "/",
]);

/** 一段纯文本里可以当分隔符的标点（**不含空格**，但含制表符 —— 表格复制时常拿它当列边界）。 */
const PLAIN_SEPARATORS = /[,，;；、\n\t]+/;

const MD_LINK = /\[([^\]]*)\]\(([^)]*)\)/g;
const TAG_IN_URL = /\/tag\/([^\s"'<>()[\]]+)/i;
/** 只有「整行就是一段以冒号收尾的分类头」才认，避免把标签名里的冒号当分类。 */
const CATEGORY_COLON = /^([A-Za-z][A-Za-z0-9_-]*)\s*[:：]\s*$/;
const CATEGORY_LOOSE_COLON = /^([^\s|:：]{1,24})\s*[:：]\s*$/;
const CATEGORY_BARE = /^([A-Za-z][A-Za-z0-9_-]*)$/;
const TABLE_DIVIDER = /^\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?$/;
/** 转义竖线的临时占位，切完单元格再还原。 */
const PIPE_PLACEHOLDER = "\u0001";

function normalizeInput(raw: string): string {
  return htmlTablesToText(
    raw
      .replace(/^\uFEFF/, "")
      .replace(/\r\n?/g, "\n")
      .replace(/[\u200B-\u200D\uFEFF]/g, "")
      .replace(/｜/g, "|"),
  );
}

/**
 * 把 HTML 表格拍平成管道分隔的纯文本。
 *
 * 从网页复制时剪贴板里常常同时带着 `text/html`，用户也可能整段贴源码。
 * 这里不用 DOMParser（保持纯函数、可在 Node 下直测），只做四件事：
 * `</td>`/`</th>` → ` | `、`</tr>` → 换行、其余标签丢弃、常见实体还原。
 * 丢标签的同时也丢了 `href`，但链接文字本来就是标签名的主体，够用。
 */
function htmlTablesToText(raw: string): string {
  if (!/<t[rhd][\s>]/i.test(raw)) return raw;
  return raw
    .replace(/<\s*br\s*\/?>/gi, "\n")
    .replace(/<\s*\/\s*(td|th)\s*>/gi, " | ")
    .replace(/<\s*\/\s*tr\s*>/gi, " |\n")
    .replace(/<[^>]+>/g, "")
    .replace(/&nbsp;/gi, " ")
    .replace(/&amp;/gi, "&")
    .replace(/&lt;/gi, "<")
    .replace(/&gt;/gi, ">")
    .replace(/&quot;/gi, '"')
    .replace(/&#0?39;/g, "'");
}

function collapseWhitespace(text: string): string {
  return text.replace(/[\s\u00a0]+/g, " ").trim();
}

/**
 * 单元格内的反转义：`\|` → `|`、`\_` → `_`、`\*` → `*`、`\\` → `\`。
 * 只处理反斜杠后紧跟这些字符的情况，标签里的普通反斜杠原样保留。
 */
function unescapeCell(cell: string): string {
  return cell.replace(/\\([|_*`\\[\]()])/g, "$1");
}

/** 去掉成对的强调标记：`**x**` / `*x*` / `__x__` / `` `x` ``。 */
function stripEmphasis(text: string): string {
  let value = text.trim();
  for (let round = 0; round < 3; round += 1) {
    const before = value;
    value = value.replace(/^(`{1,3})([\s\S]*?)\1$/, "$2");
    value = value.replace(/^(\*{1,3})([\s\S]*?)\1$/, "$2");
    value = value.replace(/^~~([\s\S]*?)~~$/, "$1");
    if (value === before) break;
  }
  // 只剥「整串被下划线包住」的情况，`big_breasts` 里的下划线不是强调。
  if (value.length > 2 && value.startsWith("_") && value.endsWith("_")) {
    const inner = value.slice(1, -1);
    if (!inner.includes("_")) value = inner;
  }
  return value.trim();
}

function isFiller(cell: string): boolean {
  const key = collapseWhitespace(cell.replace(/\*/g, "").replace(/`/g, ""))
    .toLowerCase();
  return FILLER_CELLS.has(key);
}

/**
 * 去掉尾部的标点与**不成对**的收尾括号。
 *
 * 为什么要判括号配对：`.../tag/character:kafka%28honkai%29` 解开后是
 * `kafka(honkai)`，末尾那个 `)` 是标签的一部分；而 `[**x**](url)` 多余出来的
 * `)` 不是。只有「右括号比左括号多」时才砍。
 */
function trimTrailingJunk(text: string): string {
  let value = text.replace(/[.,;:，。；、]+$/u, "").trim();
  const pairs: [string, string][] = [
    ["(", ")"],
    ["[", "]"],
    ["（", "）"],
  ];
  let changed = true;
  while (changed && value) {
    changed = false;
    for (const [open, close] of pairs) {
      if (!value.endsWith(close)) continue;
      const opens = value.split(open).length - 1;
      const closes = value.split(close).length - 1;
      if (closes > opens) {
        value = value.slice(0, -1).trim();
        changed = true;
      }
    }
  }
  return value;
}

/**
 * 解码 `/tag/` 后面的 token。
 *
 * 顺序很重要：先把 `+` 换成 `%20` 再 decodeURIComponent —— 这样 `%2B`（字面加号）
 * 不会被误当成空格；`%28` / `%29`（括号）能正确还原。
 */
export function decodeExhTagToken(token: string): string {
  let decoded = token.replace(/\+/g, "%20");
  try {
    decoded = decodeURIComponent(decoded);
  } catch {
    // 编码不完整（半个 %XX）就退回手工替换，至少别丢字符
    decoded = token.replace(/\+/g, " ");
  }
  return trimTrailingJunk(decoded);
}

/** token → 标签名：去掉命名空间前缀，压空白。`female:big breasts` → `big breasts`。 */
function nameFromToken(token: string): string | null {
  const decoded = decodeExhTagToken(token);
  const colon = decoded.indexOf(":");
  const name = colon >= 0 ? decoded.slice(colon + 1) : decoded;
  const cleaned = collapseWhitespace(name);
  return cleaned || null;
}

/** 从链接 URL 里抠出标签名（`.../tag/female:big+breasts` → `big breasts`）。 */
function tagNameFromUrl(url: string): string | null {
  const match = TAG_IN_URL.exec(url);
  if (!match) return null;
  return nameFromToken(match[1]);
}

/** 一些 ExHentai 表格里链接文本是数字（评论数、页数），这种要用 URL 兜底。 */
function isPlaceholderText(text: string): boolean {
  return text === "" || /^[\d\s.,·•+\-/()]+$/.test(text);
}

function linkToTag(linkText: string, url: string): string | null {
  const fromText = collapseWhitespace(stripEmphasis(linkText));
  if (!isPlaceholderText(fromText)) return cleanTagName(fromText);
  const fromUrl = tagNameFromUrl(url);
  return fromUrl ? cleanTagName(fromUrl) : null;
}

/** 标签名收尾清洗：压空白、去首尾标点，但不动词与词之间的空格。 */
function cleanTagName(raw: string): string | null {
  let name = collapseWhitespace(raw);
  name = name.replace(/^[,，;；、:：]+|[,，;；、:：]+$/g, "").trim();
  if (!name) return null;
  return name;
}

/** 把一段（不含链接的）纯文本切成标签：只按标点切，绝不按空格切。 */
function splitPlainRun(text: string): string[] {
  if (!text) return [];
  const cleaned = stripEmphasis(text.replace(/[*`]/g, ""));
  if (!cleaned || isFiller(cleaned)) return [];
  return cleaned
    .split(PLAIN_SEPARATORS)
    .map((part) => cleanTagName(part))
    .filter((part): part is string => Boolean(part));
}

interface CellTags {
  tags: string[];
  /** 这一格是空的 / 只有占位符 —— 计入 emptyCells。 */
  empty: boolean;
}

/**
 * 从一个单元格里把标签全部挖出来。
 *
 * 走一遍字符串，链接与纯文本按出现顺序交替产出，**顺序天然保持原文顺序**；
 * 链接边界就是标签边界，所以 `[a](x)[b](y)` 这种挨着写的也不会粘成一个。
 */
function extractCellTags(rawCell: string): CellTags {
  const cell = unescapeCell(rawCell);
  const tags: string[] = [];
  let cursor = 0;
  let seenLink = false;

  MD_LINK.lastIndex = 0;
  let match: RegExpExecArray | null = MD_LINK.exec(cell);
  while (match) {
    seenLink = true;
    tags.push(...splitPlainRun(cell.slice(cursor, match.index)));
    const name = linkToTag(match[1], match[2]);
    if (name) tags.push(name);
    cursor = match.index + match[0].length;
    match = MD_LINK.exec(cell);
  }

  if (!seenLink) {
    // 没有合法链接：先试裸 URL（`.../tag/x:y`），连残缺的 `[**x**(url)` 也能救回来；
    // 一格里有多个 URL 时按出现顺序全收；一个都没有才当纯文本切。
    const urls = [...cell.matchAll(/\/tag\/([^\s"'<>()[\]]+)/gi)];
    if (urls.length > 0) {
      for (const url of urls) {
        const name = nameFromToken(url[1]);
        if (name) tags.push(name);
      }
    } else {
      tags.push(...splitPlainRun(cell));
    }
  } else {
    tags.push(...splitPlainRun(cell.slice(cursor)));
  }

  if (!seenLink && tags.length === 0 && isFiller(cell)) {
    return { tags: [], empty: true };
  }
  const unique = dedupeInCell(tags);
  return { tags: unique, empty: unique.length === 0 };
}

/** 同一格里重复写了两遍的标签没必要留两份（跨格/跨行去重交给主流程）。 */
function dedupeInCell(tags: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const tag of tags) {
    const key = tag.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(tag);
  }
  return out;
}

/**
 * 按未转义的 `|` 切单元格。返回 null 表示「这一行不是表格行」。
 *
 * `\|` 是 Markdown 里转义的竖线，属于标签内容（少见但存在），先换占位符再还原。
 */
export function splitTableRow(line: string): string[] | null {
  if (!line.includes("|")) return null;

  const cells: string[] = [];
  let current = "";
  let pipes = 0;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (char === "\\" && line[index + 1] === "|") {
      current += PIPE_PLACEHOLDER;
      index += 1;
      continue;
    }
    if (char === "|") {
      pipes += 1;
      cells.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  cells.push(current);

  if (pipes < 2) return null;
  if (cells.length > 0 && cells[0].trim() === "") cells.shift();
  if (cells.length > 0 && cells[cells.length - 1].trim() === "") cells.pop();
  if (cells.length === 0) return null;
  if (cells.every((cell) => TABLE_DIVIDER.test(cell.trim()) || /^:?-{2,}:?$/.test(cell.trim()))) {
    return null;
  }
  return cells.map((cell) => cell.split(PIPE_PLACEHOLDER).join("|"));
}

/** 判断一格里写的是不是分类名。返回归一化后的分类键，不是分类则 null。 */
export function detectCategoryCell(
  cell: string,
  normalizeCase = true,
): string | null {
  const raw = collapseWhitespace(unescapeCell(cell).replace(/[*`]/g, ""));
  if (!raw) return null;

  const withColon = CATEGORY_COLON.exec(raw) ?? CATEGORY_LOOSE_COLON.exec(raw);
  if (withColon) {
    const key = withColon[1].trim();
    return normalizeCase ? key.toLowerCase() : key;
  }

  const bare = CATEGORY_BARE.exec(raw);
  if (bare) {
    const key = bare[1].toLowerCase();
    return KNOWN_CATEGORY_KEYS.has(key) ? key : null;
  }
  return null;
}

function truncate(text: string, limit = 40): string {
  const clean = collapseWhitespace(text);
  return clean.length > limit ? `${clean.slice(0, limit)}…` : clean;
}

/**
 * 把 Markdown 表格行就地转成 `分类: 标签1\t标签2` 行，非表格行原样返回。
 *
 * 这是给 `tags.ts:parseTagInput` 用的桥：那边已经有成熟的分隔符切分
 * （`TAG_SEPARATORS` 含制表符）和大类映射，只要把表格变成它能吃的形状就行。
 * 用 `\t` 而不用 `,` 连接 —— 标签名里可能有逗号（`a, b`），但不可能有制表符。
 *
 * `knownCategories` 是「下游认识哪些分类」的白名单。不在名单里的分类（比如
 * `location`）如果照样写 `location: a`，下游会把它当成一个带冒号的标签名，
 * 反而更脏；所以这种分类只留标签、不留头。传空即全部保留分类头。
 */
export function markdownTablesToTagLines(
  raw: string,
  knownCategories?: readonly string[],
): string {
  if (!raw.includes("|")) return raw;

  const known = knownCategories
    ? new Set(knownCategories.map((key) => key.toLowerCase()))
    : null;

  const out: string[] = [];
  for (const line of normalizeInput(raw).split("\n")) {
    const text = line.trim();
    if (!text) {
      out.push(line);
      continue;
    }
    if (TABLE_DIVIDER.test(text)) continue;

    const cells = splitTableRow(text);
    if (!cells) {
      out.push(line);
      continue;
    }

    const category = detectCategoryCell(cells[0]);
    const keepHeader = category !== null && (!known || known.has(category));
    const rest = category ? cells.slice(1) : cells;
    const names: string[] = [];
    for (const cell of rest) {
      names.push(...extractCellTags(cell).tags);
    }
    if (names.length === 0) {
      if (keepHeader) out.push(`${category}:`);
      continue;
    }
    out.push(keepHeader ? `${category}: ${names.join("\t")}` : names.join("\t"));
  }
  return out.join("\n");
}

interface MutableBlock {
  key: string;
  label: string;
  tags: string[];
}

/**
 * 主解析入口。
 *
 * 行级分流（顺序即优先级）：
 *  1. 空行 / 代码围栏 / 表格分隔行 → 跳过
 *  2. 含 ≥2 个未转义 `|` → 表格行：第 0 格认分类，其余格挖标签
 *  3. 形如 `ns: 内容` → 旧格式行（从 ExHentai 复制成纯文本时常见）
 *  4. 其余 → 整行当标签，归到上一个分类
 */
export function parseExHentaiTags(
  input: string,
  options: ParseOptions = {},
): ParseResult {
  const {
    dedupe = true,
    dedupeScope = "category",
    tagSeparator = "，",
    blockSeparator = "\n\n",
    includeCategories,
    excludeCategories,
    normalizeCategoryCase = true,
    fallbackCategory = "未分类",
  } = options;

  const errors: ParseError[] = [];
  const statistics: ParseStatistics = {
    categoryCount: 0,
    tagCount: 0,
    rowCount: 0,
    duplicatesRemoved: 0,
    emptyCells: 0,
    overlong: 0,
  };

  const pushError = (error: ParseError) => {
    // 提示最多留 20 条，避免一屏全是重复警告
    if (errors.length < 20) errors.push(error);
  };

  const text = normalizeInput(input);
  if (!text.trim()) {
    return {
      success: false,
      blocks: [],
      categories: {},
      plainText: "",
      statistics,
      errors: [{ code: "EMPTY_INPUT", message: "还没有粘贴内容" }],
    };
  }

  const blockMap = new Map<string, MutableBlock>();
  const globalSeen = new Set<string>();
  const categorySeen = new Map<string, Set<string>>();
  const include = includeCategories ? new Set(includeCategories.map((key) => key.toLowerCase())) : null;
  const exclude = excludeCategories ? new Set(excludeCategories.map((key) => key.toLowerCase())) : null;

  let lastCategory: string | null = null;

  const ensureBlock = (key: string): MutableBlock | null => {
    const lower = key.toLowerCase();
    if (include && !include.has(lower)) return null;
    if (exclude && exclude.has(lower)) return null;
    const existing = blockMap.get(key);
    if (existing) return existing;
    const created: MutableBlock = { key, label: key, tags: [] };
    blockMap.set(key, created);
    return created;
  };

  const addTag = (rawName: string, category: string, line: number) => {
    const name = cleanTagName(rawName);
    if (!name) return;
    if ([...name].length > MAX_TAG_NAME_LENGTH) {
      statistics.overlong += 1;
      pushError({
        code: "OVERLONG_TAG",
        message: `标签超过 ${MAX_TAG_NAME_LENGTH} 个字符，已跳过`,
        line,
        sample: truncate(name),
      });
      return;
    }
    const block = ensureBlock(category);
    if (!block) return;

    const key = name.toLowerCase();
    if (dedupe) {
      if (dedupeScope === "global") {
        if (globalSeen.has(key)) {
          statistics.duplicatesRemoved += 1;
          return;
        }
        globalSeen.add(key);
      } else {
        let seen = categorySeen.get(category);
        if (!seen) {
          seen = new Set<string>();
          categorySeen.set(category, seen);
        }
        if (seen.has(key)) {
          statistics.duplicatesRemoved += 1;
          return;
        }
        seen.add(key);
      }
    }
    block.tags.push(name);
  };

  const lines = text.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const lineNumber = index + 1;
    const line = lines[index];
    const trimmed = line.trim();
    if (!trimmed) continue;
    if (trimmed.startsWith("```")) continue;
    if (TABLE_DIVIDER.test(trimmed)) continue;

    const cells = splitTableRow(trimmed);
    if (cells) {
      statistics.rowCount += 1;
      const category = detectCategoryCell(cells[0], normalizeCategoryCase);
      const tagCells = category ? cells.slice(1) : cells;
      const rowCategory = category ?? lastCategory ?? fallbackCategory;
      if (category) lastCategory = category;
      else if (!lastCategory) {
        lastCategory = fallbackCategory;
        pushError({
          code: "ORPHAN_ROW",
          message: "这一行没有分类单元格，已归到「未分类」",
          line: lineNumber,
          sample: truncate(trimmed),
        });
      }

      let produced = 0;
      for (const cell of tagCells) {
        const extracted = extractCellTags(cell);
        if (extracted.empty) {
          statistics.emptyCells += 1;
          continue;
        }
        for (const name of extracted.tags) {
          addTag(name, rowCategory, lineNumber);
          produced += 1;
        }
      }
      if (produced === 0) {
        pushError({
          code: "EMPTY_ROW",
          message: "这一行没有可用的标签",
          line: lineNumber,
          sample: truncate(trimmed),
        });
      }
      continue;
    }

    // 单独一行只有分类名（网页复制成「一类一行、标签各占一行」时会这样）
    const loneCategory = detectCategoryCell(trimmed, normalizeCategoryCase);
    if (loneCategory) {
      lastCategory = loneCategory;
      continue;
    }

    // 旧格式：`female: big breasts, sole female`
    const header = /^([^\s,，;；、:：]{1,24})\s*[:：]\s*([\s\S]*)$/.exec(trimmed);
    if (header && header[2].trim()) {
      const key = normalizeCategoryCase ? header[1].toLowerCase() : header[1];
      lastCategory = key;
      statistics.rowCount += 1;
      const pieces = splitPlainRun(header[2]);
      for (const piece of pieces) addTag(piece, key, lineNumber);
      continue;
    }

    // 裸标签行：整行当标签，归上一个分类（不按空格切）
    const bare = stripEmphasis(trimmed);
    if (bare && !isFiller(bare)) {
      const category = lastCategory ?? fallbackCategory;
      if (!lastCategory) {
        lastCategory = fallbackCategory;
        pushError({
          code: "ORPHAN_ROW",
          message: "这一行没有分类，已归到「未分类」",
          line: lineNumber,
          sample: truncate(trimmed),
        });
      }
      statistics.rowCount += 1;
      for (const name of splitPlainRun(bare)) addTag(name, category, lineNumber);
    }
  }

  const blocks: ParsedBlock[] = [...blockMap.values()]
    .filter((block) => block.tags.length > 0)
    .map((block) => ({ key: block.key, label: block.label, tags: block.tags }));

  const categories: Record<string, string[]> = {};
  for (const block of blocks) categories[block.key] = block.tags;

  statistics.categoryCount = blocks.length;
  statistics.tagCount = blocks.reduce((sum, block) => sum + block.tags.length, 0);

  const plainText = blocks
    .map((block) => `${block.key}: ${block.tags.join(tagSeparator)}`)
    .join(blockSeparator);

  if (statistics.tagCount === 0) {
    pushError({
      code: "NO_TAGS",
      message: "没有识别到任何标签，确认一下粘贴的是不是 ExHentai 的标签表格",
    });
  }

  const unclosed = (text.match(/\[/g)?.length ?? 0) - (text.match(/\]/g)?.length ?? 0);
  if (unclosed > 0) {
    pushError({
      code: "UNCLOSED_BRACKET",
      message: "有没闭合的方括号，链接文本可能没取全",
    });
  }

  return {
    success: statistics.tagCount > 0,
    blocks,
    categories,
    plainText,
    statistics,
    errors,
  };
}
