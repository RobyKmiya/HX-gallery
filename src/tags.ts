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

/** 按 Unicode 码点计数，与后端 Rust 的 `chars().count()` 对齐。 */
function codePointLength(text: string): number {
  return [...text].length;
}

export function parseTagInput(raw: string): TagParseResult {
  const tags: ParsedTag[] = [];
  const rejected: RejectedTag[] = [];
  const seen = new Set<string>();
  let category: string | null = null;

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
        for (const part of header[2].split(TAG_SEPARATORS)) {
          push(part, category);
        }
        continue;
      }
    }

    for (const part of text.split(TAG_SEPARATORS)) {
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
