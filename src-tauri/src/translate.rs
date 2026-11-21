//! 标签词典：固定含义的词条翻译 + 中文归一。
//!
//! 两条规矩（管理员 2026-11-22 明确）：
//! 1. **只翻固定含义的词**——大类别（female / male / mixed / other）、语言、完成度、
//!    常见体型与行为词；角色名、社团名、画师名这类专有名词不翻，翻了反而看不懂。
//! 2. **中文输入要落到同一条英文标签上**——贴「女性」不该再建一条「女性」标签，
//!    而是归一到 `female`，显示时用词典里的中文译名，看起来就是一条。

/// (规范英文名, 中文译名)。顺序无所谓，查找时统一归一化后比对。
const ENTRIES: &[(&str, &str)] = &[
    // ── 大类别（也用作命名空间）──
    ("female", "女性"),
    ("male", "男性"),
    ("mixed", "混合"),
    ("other", "其他"),
    ("futanari", "扶她"),
    ("tomgirl", "伪娘"),
    ("crossdressing", "变装"),
    // ── 人数 / 关系 ──
    ("1girl", "一女"),
    ("1boy", "一男"),
    ("2girls", "两女"),
    ("3girls", "三女"),
    ("4girls", "四女"),
    ("6girls", "六女"),
    ("sole female", "仅有女性"),
    ("sole male", "仅有男性"),
    ("multiple girls", "多女"),
    ("multiple boys", "多男"),
    ("yuri", "百合"),
    ("yaoi", "耽美"),
    ("harem", "后宫"),
    ("vanilla", "纯爱"),
    ("netorare", "NTR"),
    ("cheating", "出轨"),
    ("incest", "近亲"),
    ("group", "群"),
    // ── 体型 ──
    ("big breasts", "巨乳"),
    ("huge breasts", "超巨乳"),
    ("medium breasts", "中等胸"),
    ("small breasts", "贫乳"),
    ("flat chest", "平胸"),
    ("muscular", "肌肉"),
    ("chubby", "微胖"),
    ("slim", "纤细"),
    ("tall", "高个"),
    ("petite", "娇小"),
    // ── 行为 ──
    ("nakadashi", "内射"),
    ("creampie", "内射"),
    ("paizuri", "乳交"),
    ("fellatio", "口交"),
    ("deepthroat", "深喉"),
    ("anal", "肛交"),
    ("handjob", "手交"),
    ("masturbation", "自慰"),
    ("ahegao", "阿黑颜"),
    ("bondage", "捆绑"),
    ("shibari", "绳缚"),
    ("bdsm", "BDSM"),
    ("tentacles", "触手"),
    ("pregnant", "怀孕"),
    ("lactation", "哺乳"),
    ("exhibitionism", "露出"),
    ("kiss", "接吻"),
    ("hug", "拥抱"),
    // ── 服饰 ──
    ("stockings", "丝袜"),
    ("pantyhose", "连裤袜"),
    ("thighhighs", "大腿袜"),
    ("lingerie", "内衣"),
    ("panties", "内裤"),
    ("bra", "胸罩"),
    ("bikini", "比基尼"),
    ("swimsuit", "泳装"),
    ("school uniform", "校服"),
    ("serafuku", "水手服"),
    ("maid", "女仆"),
    ("nurse", "护士"),
    ("kimono", "和服"),
    ("chinese dress", "旗袍"),
    ("dress", "连衣裙"),
    ("skirt", "裙子"),
    ("glasses", "眼镜"),
    ("mask", "口罩"),
    ("weapon", "武器"),
    ("armor", "铠甲"),
    // ── 外貌 ──
    ("long hair", "长发"),
    ("short hair", "短发"),
    ("twintails", "双马尾"),
    ("ponytail", "马尾"),
    ("blonde hair", "金发"),
    ("black hair", "黑发"),
    ("white hair", "白发"),
    ("silver hair", "银发"),
    ("red hair", "红发"),
    ("blue hair", "蓝发"),
    ("pink hair", "粉发"),
    ("heterochromia", "异色瞳"),
    ("blush", "脸红"),
    ("smile", "微笑"),
    ("crying", "哭泣"),
    ("sleeping", "睡姿"),
    ("tanlines", "晒痕"),
    ("dark skin", "深色皮肤"),
    // ── 题材 / 风格 ──
    ("full color", "全彩"),
    ("monochrome", "单色"),
    ("text cleaned", "已修文字"),
    ("sample", "样张"),
    ("artbook", "画集"),
    ("anthology", "选集"),
    ("magazine", "杂志"),
    ("webtoon", "条漫"),
    ("comic", "漫画"),
    ("manga", "日漫"),
    ("doujinshi", "同人志"),
    ("novel", "小说"),
    ("illustration", "插画"),
    ("photoset", "写真"),
    ("game cg", "游戏CG"),
    ("action", "动作"),
    ("comedy", "喜剧"),
    ("drama", "剧情"),
    ("fantasy", "奇幻"),
    ("sci fi", "科幻"),
    ("horror", "恐怖"),
    ("romance", "恋爱"),
    ("slice of life", "日常"),
    ("school", "校园"),
    ("historical", "历史"),
    ("military", "军事"),
    ("sports", "运动"),
    // ── 通用题材（画匣不只放本子，风景/插画类也要能看懂）──
    ("landscape", "风景"),
    ("scenery", "风景"),
    ("sunset", "日落"),
    ("sunrise", "日出"),
    ("nature", "自然"),
    ("sky", "天空"),
    ("night", "夜晚"),
    ("city", "城市"),
    ("portrait", "人像"),
    ("scenery photography", "风景摄影"),
    // ── 元信息（语言 / 状态）──
    ("chinese", "中文"),
    ("english", "英文"),
    ("japanese", "日文"),
    ("korean", "韩文"),
    ("translated", "已翻译"),
    ("rewrite", "重绘文字"),
    ("ai generated", "AI生成"),
    ("animated", "动图"),
    ("uncensored", "无码"),
    ("full censorship", "全码"),
    ("variant set", "差分套组"),
    // ── 补充：库里实际出现过的固定词（2026-11-22 按真实标签表补）──
    ("angel", "天使"),
    ("bride", "新娘"),
    ("business suit", "西装"),
    ("butt plug", "肛塞"),
    ("diaper", "尿布"),
    ("doctor", "医生"),
    ("domination loss", "败北"),
    ("gag", "塞口"),
    ("gaping", "张开"),
    ("garter belt", "吊袜带"),
    ("handicapped", "残缺"),
    ("leg lock", "腿锁"),
    ("masked face", "遮脸"),
    ("mouth mask", "口罩"),
    ("multimouth blowjob", "多人口交"),
    ("multiple handjob", "多人手交"),
    ("multiple paizuri", "多人乳交"),
    ("muscles", "肌肉"),
    ("netorase", "主动寝取"),
    ("niece", "侄女"),
    ("nose hook", "鼻钩"),
    ("petplay", "宠物调教"),
    ("priestess", "巫女"),
    ("schoolboy uniform", "男生校服"),
    ("sockjob", "袜足交"),
    ("spanking", "打屁股"),
    ("tomboy", "假小子"),
    ("voyeurism", "偷窥"),
    ("wolf girl", "狼娘"),
    ("kemonomimi", "兽耳"),
    ("magical girl", "魔法少女"),
];

/// 词典全表，供「回填已有标签译名」这类维护动作遍历。
pub fn dictionary() -> &'static [(&'static str, &'static str)] {
    ENTRIES
}

/// 统一写法：小写、下划线/连字符当空格、压掉多余空白。
fn normalize_key(name: &str) -> String {
    name
        .to_ascii_lowercase()
        .replace('_', " ")
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 英文标签 → 中文译名（词典里没有就返回 None，交给 AI 批量翻译或人工填）。
pub fn translate_tag(name: &str) -> Option<&'static str> {
    let key = normalize_key(name);
    ENTRIES
        .iter()
        .find(|(english, _)| *english == key)
        .map(|(_, chinese)| *chinese)
}

/// 中文 → 规范英文名。
///
/// 只做「中文 → 英文」这一向：英文标签保持用户输入的原样（大小写不动），
/// 免得贴个 `Landscape` 被改写成 `landscape`，把原始写法弄丢。
pub fn canonical_tag_name(name: &str) -> Option<&'static str> {
    let key = normalize_key(name);
    ENTRIES
        .iter()
        .find(|(_, chinese)| *chinese == key)
        .map(|(english, _)| *english)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_namespaces_and_fixed_words() {
        assert_eq!(translate_tag("female"), Some("女性"));
        assert_eq!(translate_tag("FEMALE"), Some("女性"));
        assert_eq!(translate_tag("big_breasts"), Some("巨乳"));
        assert_eq!(translate_tag("Big Breasts"), Some("巨乳"));
        assert_eq!(translate_tag("full color"), Some("全彩"));
        // 专有名词不翻
        assert_eq!(translate_tag("some artist name"), None);
    }

    #[test]
    fn canonicalizes_chinese_input() {
        assert_eq!(canonical_tag_name("女性"), Some("female"));
        assert_eq!(canonical_tag_name("巨乳"), Some("big breasts"));
        assert_eq!(canonical_tag_name("全彩"), Some("full color"));
        // 英文标签不改写（保持用户写法）
        assert_eq!(canonical_tag_name("big_breasts"), None);
        assert_eq!(canonical_tag_name("Landscape"), None);
        // 词典外的中文原样保留
        assert_eq!(canonical_tag_name("某个角色名"), None);
    }
}
