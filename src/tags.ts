/**
 * 批量标签解析。
 *
 * 目标是能吃下两种最常见的「标准 tag 格式」：
 *
 * 1. 带命名空间（E-Hentai / ExHentai 复制出来的原样，也是多数下载器的导出格式）：
 *      female:big breasts, sole female, long hair
 *      character:aoba utsumihikari, tachibanasensei
 *      artist:mountainhan
 * 2. 纯标签（Danbooru / Gelbooru 的逗号分隔，或本应用常见的「每行一个」）：
 *      全彩
 *      纯爱, 萝莉；AI、黑白
 *
 * 命名空间会被映射成应用里的大类（原作/角色/作者/女性/男性/混合/其他/语言），
 * 标签本身只留小标签的名字，因此导入后会直接落进对应大类，而不是掉进「未分类」。
 *
 * 三点刻意的取舍：
 * - 只认白名单里的命名空间 + 显式带 `/` 的分类路径。像 `re:从零开始的异世界生活`
 *   这种标签名里本来就带冒号，不能当成命名空间，否则会被劈成两半。
 * - 换行与制表符一律当分隔符（沿用「每行一个」的老习惯）。从网页复制时若在长标签
 *   中间折行，会切出一个碎片标签 —— 这是可见、可删的，比整段粘成一坨更好收拾。
 *   制表符必须切开：E-Hentai 的 `female://thorse girl\tsole female` 若整段当成一个
 *   标签，会脏成 `female://thorse girl…` 这种带命名空间前缀的怪名字。
 * - 名字里的下划线换成空格（Danbooru 用 `long_hair` 表示「long hair」这一个标签）。
 */

export const MAX_TAG_NAME_LENGTH = 100;

/** 命名空间 → 应用内大类。键统一小写比较。 */
const NAMESPACE_CATEGORIES: Record<string, string> = {
  language: "语言",
  lang: "语言",
  语言: "语言",

  parody: "原作",
  copyright: "原作",
  series: "原作",
  work: "原作",
  source: "原作",
  原作: "原作",
  作品: "原作",
  出处: "原作",

  character: "角色",
  characters: "角色",
  char: "角色",
  角色: "角色",
  人物: "角色",

  artist: "作者",
  authors: "作者",
  group: "作者",
  circle: "作者",
  studio: "作者",
  作者: "作者",
  画师: "作者",
  社团: "作者",

  female: "女性",
  girl: "女性",
  females: "女性",
  女性: "女性",

  male: "男性",
  boy: "男性",
  males: "男性",
  男性: "男性",

  mixed: "混合",
  混合: "混合",

  other: "其他",
  misc: "其他",
  meta: "其他",
  cosplay: "其他",
  reclass: "其他",
  temp: "其他",
  species: "其他",
  general: "其他",
  lore: "其他",
  其他: "其他",
  杂项: "其他",
};

/** 标签之间的分隔符：半角/全角逗号、分号、顿号，以及制表符。 */
export const TAG_SEPARATORS = /[,，;；、\t]+/;

/** `命名空间: 内容`。冒号前不允许出现分隔符，避免把整个逗号列表误判成头。 */
const HEADER_PATTERN = /^([^,，;；、:：]{1,24})\s*[:：]\s*(.*)$/;

export interface ParsedTag {
  name: string;
  category: string | null;
}

export interface RejectedTag {
  name: string;
  reason: string;
}

export interface TagParseResult {
  tags: ParsedTag[];
  /** 认出来但无法导入的（例如超长），用来提示而不是静默丢弃。 */
  rejected: RejectedTag[];
}

function normalizeTagName(raw: string): string {
  let name = raw.trim();
  name = name.replace(/^#+/, "").trim();
  name = name.replace(/^["'“”‘’]+|["'“”‘’]+$/g, "").trim();
  // Danbooru 标签用下划线连接词组，界面上用空格更自然。
  name = name.replace(/_/g, " ");
  // 还原 `\(` `\)` 这类转义，避免标签里出现反斜杠。
  name = name.replace(/\\([()\\])/g, "$1");
  return name.replace(/\s+/g, " ").trim();
}


/** 分类 → 命名空间键的反向映射（每类取第一个键，保证回贴可解析）。 */
const CATEGORY_NAMESPACES: Record<string, string> = (() => {
  const map: Record<string, string> = {};
  for (const [key, value] of Object.entries(NAMESPACE_CATEGORIES)) {
    if (!(value in map)) map[value] = key;
  }
  return map;
})();

/** 把标签列表序列化成可回贴的文本：`命名空间: t1, t2` 分行；未分类的裸行放最后。 */
export function tagsToCopyText(
  tags: { name: string; category: string | null }[],
): string {
  const grouped = new Map<string, string[]>();
  const loose: string[] = [];
  for (const tag of tags) {
    if (tag.category && CATEGORY_NAMESPACES[tag.category]) {
      const key = CATEGORY_NAMESPACES[tag.category];
      const list = grouped.get(key) ?? [];
      list.push(tag.name);
      grouped.set(key, list);
    } else {
      loose.push(tag.name);
    }
  }
  const lines: string[] = [];
  for (const [key, list] of grouped) {
    if (list.length > 0) lines.push(`${key}: ${list.join(", ")}`);
  }
  if (loose.length > 0) lines.push(loose.join(", "));
  return lines.join("\n");
}

/** 按 Unicode 码点计数，与后端 Rust 的 `chars().count()` 对齐。 */function codePointLength(text: string): number {
  return [...text].length;
}

/**
 * 词库切分：一段没有标点的文本（例如从 exhentai 页面整段复制的标签块），
 * 按已有标签词典做贪心最长匹配。命中段输出词典里的规范名（大小写/下划线
 * 自动归一，能直接挂上既有标签）；连续未命中的词合并为一个标签保留原文，
 * 整段零命中时整段作为一个标签（与旧行为一致，不丢内容）。
 */
function segmentByDictionary(text: string, dictionary: string[] | null): string[] {
  if (!dictionary || dictionary.length === 0) return [text.trim()].filter(Boolean);
  const source = text.trim();
  if (!source) return [];
  const found: string[] = [];
  let cursor = 0;
  while (cursor < source.length) {
    while (cursor < source.length && /\s/.test(source[cursor])) cursor += 1;
    if (cursor >= source.length) break;
    let hit: string | null = null;
    for (const name of dictionary) {
      if (
        name.length <= source.length - cursor &&
        source.slice(cursor, cursor + name.length).toLowerCase() === name.toLowerCase()
      ) {
        hit = name;
        break;
      }
    }
    if (hit) {
      found.push(hit);
      cursor += hit.length;
      continue;
    }
    // 未命中：吃一个词；连续未命中的词合并为一个标签（避免丢内容）
    const wordStart = cursor;
    let end = source.length;
    for (let probe = cursor + 1; probe < source.length; probe += 1) {
      if (/\s/.test(source[probe])) {
        // 试探：从这里往后还有词典命中吗？有就在命中处截断
        const rest = source.slice(probe).trimStart();
        let futureHit = false;
        for (const name of dictionary) {
          if (rest.toLowerCase().startsWith(name.toLowerCase())) {
            futureHit = true;
            break;
          }
        }
        if (futureHit) {
          end = probe;
          break;
        }
      }
    }
    const chunk = source.slice(wordStart, end).trim();
    if (chunk) found.push(chunk);
    cursor = wordStart + chunk.length || cursor + 1;
    if (chunk.length === 0) cursor += 1;
  }
  return found;
}

/**
 * 从「富文本粘贴」里无损还原标签边界。
 *
 * 从网页或 AI 对话里复制标签块时，每个 tag 往往是一个独立链接（`<a>`）。浏览器转成
 * 纯文本会把它们黏在一起（`big breasts business suit`），这时只能靠词库去猜；
 * 但 HTML 里链接边界本来就是清晰的——把每个 `<a>` 的内文抽出来、之间补逗号，再交给
 * `parseTagInput`，就是无损切分，连词库都不需要命中。
 *
 * 返回 null 表示这段 HTML 不像标签块（链接少于 2 个），调用方应退回普通文本处理。
 */
export function tagsFromHtml(html: string): string | null {
  if (!html || !/<a[\s>]/i.test(html)) return null;
  const doc = new DOMParser().parseFromString(html, "text/html");
  const links = doc.body.querySelectorAll("a");
  if (links.length < 2) return null;

  const BLOCKS = new Set([
    "p", "div", "li", "tr", "ul", "ol", "table", "h1", "h2", "h3", "h4", "h5", "h6",
  ]);
  const walk = (node: Node): string => {
    if (node.nodeType === 3) return node.textContent ?? ""; // 文本节点
    if (node.nodeType !== 1) return "";
    const element = node as Element;
    const tag = element.tagName.toLowerCase();
    if (tag === "br") return "\n";
    const inner = Array.from(element.childNodes).map(walk).join("");
    // 链接 = 一个完整标签，用逗号收尾，让 TAG_SEPARATORS 当权威边界切。
    if (tag === "a") return inner.trim() ? `${inner.trim()},` : "";
    if (BLOCKS.has(tag)) return `\n${inner}\n`;
    return inner;
  };

  const text = Array.from(doc.body.childNodes)
    .map(walk)
    .join("")
    .replace(/,+/g, ",")
    .replace(/,\s*\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .replace(/[ \t]{2,}/g, " ")
    .trim();
  return text || null;
}

/**
 * exhentai / e-hentai 的 tag 链接（管理员 11-23）：
 * `https://exhentai.org/tag/female:kissing` 这类 URL 直接粘进来也能解析——
 * 抠出 `/tag/` 后面的 token，还原成 `ns: name` 交给正常解析流程。
 */
const TAG_URL_PATTERN = /https?:\/\/[^\s"'<>（）:]*?\/tag\/([^\s"'<>（）]+)/gi;

function decodeTagUrlToken(token: string): string {
  let decoded = token;
  try {
    decoded = decodeURIComponent(decoded);
  } catch {
    // 编码不完整就保留原样
  }
  return decoded
    .replace(/\+/g, " ")
    .replace(/[.,;，。；」』）)\]]+$/u, "")
    .trim();
}

export function parseTagInput(
  raw: string,
  knownNames?: readonly string[],
): TagParseResult {
  const tags: ParsedTag[] = [];
  const rejected: RejectedTag[] = [];
  const seen = new Set<string>();
  let category: string | null = null;

  // 先把 tag 链接换成 `ns: name` 独立行（走既有的命名空间头解析）。
  raw = raw.replace(TAG_URL_PATTERN, (_match, token: string) => {
    const decoded = decodeTagUrlToken(token);
    const colon = decoded.indexOf(":");
    if (colon <= 0) return " ";
    const ns = decoded.slice(0, colon).trim();
    const name = decoded.slice(colon + 1).trim();
    if (!ns || !name) return " ";
    return `\n${ns}: ${name}\n`;
  });
  // 词库：归一化后按长度降序（贪心最长匹配）
  const dictionary = knownNames
    ? knownNames
        .map((name) => normalizeTagName(name))
        .filter((name) => name.length > 1)
        .sort((left, right) => right.length - left.length)
    : null;

  const push = (rawName: string, currentCategory: string | null) => {
    const name = normalizeTagName(rawName);
    if (!name) return;
    if (codePointLength(name) > MAX_TAG_NAME_LENGTH) {
      rejected.push({
        name: `${name.slice(0, 24)}…`,
        reason: `超过 ${MAX_TAG_NAME_LENGTH} 个字符`,
      });
      return;
    }
    const key = name.toLowerCase();
    if (seen.has(key)) return;
    seen.add(key);
    tags.push({ name, category: currentCategory });
  };

  for (const line of raw.replace(/\r\n?/g, "\n").split("\n")) {
    const text = line.trim();
    // 空行结束当前命名空间，后面的裸标签不再继承上面的分类。
    if (!text) {
      category = null;
      continue;
    }

    const header = HEADER_PATTERN.exec(text);
    if (header) {
      const key = header[1].trim();
      const mapped = key.includes("/")
        ? key // 显式分类路径，例如 `角色/原神: 胡桃`
        : NAMESPACE_CATEGORIES[key.toLowerCase()];
      if (mapped) {
        category = mapped;
        const content = header[2].trim();
        if (!content) continue;
        if (TAG_SEPARATORS.test(content)) {
          // 有显式分隔符：逗号/分号/顿号是权威边界
          for (const part of content.split(TAG_SEPARATORS)) {
            push(part, category);
          }
        } else {
          // 无分隔符：词库最长匹配（exhentai 复制块就是这种形态）
          for (const part of segmentByDictionary(content, dictionary)) {
            push(part, category);
          }
        }
        continue;
      }
    }

    const pieces = TAG_SEPARATORS.test(text)
      ? text.split(TAG_SEPARATORS)
      : segmentByDictionary(text, dictionary);
    for (const part of pieces) {
      const trimmed = part.trim();
      // 内联命名空间 token：`female:kissing` 这种（无空格）。命中词表就直接
      // 按命名空间收，不做词库贪心切分——切分可能把名字切坏。
      const nsMatch = /^([A-Za-z_]+):(.+)$/.exec(trimmed);
      if (nsMatch) {
        const mapped = NAMESPACE_CATEGORIES[nsMatch[1].toLowerCase()];
        if (mapped) {
          push(nsMatch[2], mapped);
          continue;
        }
      }
      push(part, category);
    }
  }

  return { tags, rejected };
}

/** 按大类统计，用于导入前的预览。 */
export function summarizeByCategory(tags: ParsedTag[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const tag of tags) {
    const key = tag.category ?? "未分类";
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return [...counts.entries()];
}
