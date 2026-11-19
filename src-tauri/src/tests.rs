use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use image::{ImageFormat, Rgb, RgbImage};
use uuid::Uuid;
use zip::write::SimpleFileOptions;

use crate::db;
use crate::importer;
use crate::library;
use crate::models::{ArchiveQuery, ImportOptions, TagDraft, TagTranslationInput};

struct Fixture {
    base: PathBuf,
    library: PathBuf,
    source: PathBuf,
    covers: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("archive-gallery-test-{}", Uuid::new_v4()));
        let library = base.join("library");
        let source = base.join("source");
        let covers = base.join("covers");
        fs::create_dir_all(&library).unwrap();
        fs::create_dir_all(&source).unwrap();
        // 与真实使用顺序一致：先「打开库」（建库 + 迁移 + 建全文索引），再导入/扫描。
        // 导入与扫描现在只开工作连接、不再顺带建库，所以测试夹具必须先初始化。
        db::open_library(&library).expect("初始化测试库失败");
        Self {
            base,
            library,
            source,
            covers,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn png_bytes(color: [u8; 3]) -> Vec<u8> {
    let image = RgbImage::from_pixel(64, 96, Rgb(color));
    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn write_zip(path: &Path, names: &[&str], salt: u8) {
    let file = fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for (index, name) in names.iter().enumerate() {
        writer.start_file(*name, options).unwrap();
        writer
            .write_all(&png_bytes([
                (index as u8).wrapping_mul(31).wrapping_add(salt),
                90,
                140,
            ]))
            .unwrap();
    }

    writer.finish().unwrap();
}

fn archive_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| crate::archive::is_supported_archive(path))
        .collect()
}

#[test]
fn import_scan_and_query_pipeline() {
    let fixture = Fixture::new();

    let zip_a = fixture.source.join("A 画集.cbz");
    let zip_b = fixture.source.join("B 画集.zip");
    let zip_a_copy = fixture.source.join("A 画集 副本.cbz");
    write_zip(&zip_a, &["cover.png", "page2.png"], 20);
    write_zip(&zip_b, &["001.png", "002.png", "003.png"], 80);
    fs::copy(&zip_a, &zip_a_copy).unwrap();

    let mut progress_events = 0usize;
    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip_a.clone(), zip_b.clone()],
        ImportOptions::default(),
        |_| progress_events += 1,
    );

    assert_eq!(report.imported, 2, "errors: {:?}", report.errors);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failed, 0);
    assert!(progress_events > 0);
    assert_eq!(archive_files(&fixture.library).len(), 2);

    let context = db::open_library(&fixture.library).unwrap();

    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();

    assert_eq!(page.total, 2);
    assert_eq!(page.items.len(), 2);

    let first = &page.items[0];
    let second = &page.items[1];
    assert_eq!(first.title, "A 画集");
    assert_eq!(first.ext, "cbz");
    assert_eq!(first.page_count, 2);
    assert_eq!(first.rating, 60);
    assert!(Path::new(&first.file_path).exists());
    let cover = first.cover_path.as_ref().expect("cover missing");
    assert!(Path::new(cover).exists());
    assert!(cover.ends_with(".jpg"));

    assert_eq!(second.title, "B 画集");
    assert_eq!(second.page_count, 3);
    assert!(!second.missing);

    let zip_b_id = second.id;

    let again = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip_a_copy],
        ImportOptions::default(),
        |_| {},
    );
    assert_eq!(again.imported, 0);
    assert_eq!(again.skipped, 1);
    assert_eq!(archive_files(&fixture.library).len(), 2);

    fs::remove_file(fixture.library.join("B 画集.zip")).unwrap();
    fs::rename(
        fixture.library.join("A 画集.cbz"),
        fixture.library.join("A 画集-改名.cbz"),
    )
    .unwrap();

    let scan = library::scan_with_covers(&fixture.library, &fixture.covers);
    assert_eq!(scan.found, 1, "scan errors: {:?}", scan.errors);
    assert_eq!(scan.updated, 1);
    assert_eq!(scan.missing, 1);

    let after_scan = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    assert_eq!(after_scan.total, 2);
    let missing = after_scan
        .items
        .iter()
        .find(|item| item.file_name == "B 画集.zip")
        .expect("missing row");
    assert!(missing.missing);
    let moved = after_scan
        .items
        .iter()
        .find(|item| item.file_name == "A 画集-改名.cbz")
        .expect("moved row");
    assert!(!moved.missing);
    assert!(Path::new(&moved.file_path).exists());

    fs::copy(
        fixture.source.join("B 画集.zip"),
        fixture.library.join("B 画集.zip"),
    )
    .unwrap();

    let rescan = library::scan_with_covers(&fixture.library, &fixture.covers);
    assert_eq!(rescan.found, 2, "scan errors: {:?}", rescan.errors);
    assert_eq!(rescan.missing, 0);
    let count: i64 = context
        .conn
        .query_row(
            "SELECT COUNT(*) FROM archives WHERE missing = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);

    db::set_rating(&context.conn, zip_b_id, 95).unwrap();
    assert!(db::set_rating(&context.conn, zip_b_id, 101).is_err());
    assert!(db::set_rating(&context.conn, zip_b_id, -1).is_err());
    assert!(db::set_rating(&context.conn, i64::MAX, 50).is_err());

    let rated = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("rating".to_string()),
            direction: Some("desc".to_string()),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(rated.items[0].rating, 95);
    assert_eq!(rated.items[1].rating, 60);

    let first_page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            page: Some(1),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(first_page.page, 1);
    assert_eq!(first_page.page_size, 12);
    assert_eq!(first_page.total, 2);
}

#[test]
fn combined_filters_narrow_results() {
    let fixture = Fixture::new();

    let high = fixture.source.join("高分.cbz");
    let low = fixture.source.join("低分.cbz");
    let mid = fixture.source.join("中分.cbz");
    let gone = fixture.source.join("缺失.cbz");
    write_zip(&high, &["001.png", "002.png"], 11);
    write_zip(&low, &["001.png", "002.png", "003.png"], 31);
    write_zip(&mid, &["001.png", "002.png", "003.png", "004.png"], 61);
    write_zip(&gone, &["001.png"], 91);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![high, low, mid, gone],
        ImportOptions::default(),
        |_| {},
    );

    let context = db::open_library(&fixture.library).unwrap();
    let all = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(all.total, 4);
    let id_of = |keyword: &str| {
        all.items
            .iter()
            .find(|item| item.title.contains(keyword))
            .map(|item| item.id)
            .unwrap()
    };

    db::set_rating(&context.conn, id_of("高分"), 95).unwrap();
    db::set_rating(&context.conn, id_of("低分"), 15).unwrap();
    db::set_rating(&context.conn, id_of("中分"), 50).unwrap();
    db::set_rating(&context.conn, id_of("缺失"), 70).unwrap();
    db::set_reading_progress(&context.conn, id_of("低分"), 3).unwrap();
    db::set_reading_progress(&context.conn, id_of("中分"), 2).unwrap();
    db::set_missing(&context.conn, id_of("缺失"), true).unwrap();
    db::add_tag(&context.conn, id_of("高分"), "风景").unwrap();
    db::add_tag(&context.conn, id_of("中分"), "风景").unwrap();
    let collection = db::create_collection(&context.conn, "专题").unwrap();
    for keyword in ["高分", "中分", "缺失"] {
        db::set_archive_collection(&context.conn, id_of(keyword), collection.id, true).unwrap();
    }

    let titles = |query: ArchiveQuery| {
        library::query_archives(&context.conn, &fixture.library, &fixture.covers, &query)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.title)
            .collect::<Vec<_>>()
    };

    let rating_only = titles(ArchiveQuery {
        rating_buckets: Some(vec![5]),
        sort: Some("title".to_string()),
        direction: Some("asc".to_string()),
        ..ArchiveQuery::default()
    });
    assert_eq!(rating_only, vec!["高分"]);

    let low_rating = titles(ArchiveQuery {
        rating_buckets: Some(vec![0, 1]),
        ..ArchiveQuery::default()
    });
    assert_eq!(low_rating, vec!["低分"]);

    let finished = titles(ArchiveQuery {
        read_statuses: Some(vec!["finished".to_string()]),
        ..ArchiveQuery::default()
    });
    assert_eq!(finished, vec!["低分"]);

    let unread = titles(ArchiveQuery {
        read_statuses: Some(vec!["unread".to_string()]),
        sort: Some("title".to_string()),
        direction: Some("asc".to_string()),
        ..ArchiveQuery::default()
    });
    assert_eq!(unread, vec!["缺失", "高分"]);

    let reading = titles(ArchiveQuery {
        read_statuses: Some(vec!["reading".to_string()]),
        ..ArchiveQuery::default()
    });
    assert_eq!(reading, vec!["中分"]);

    let missing_only = titles(ArchiveQuery {
        missing_only: Some(true),
        ..ArchiveQuery::default()
    });
    assert_eq!(missing_only, vec!["缺失"]);

    let tag = db::list_tags(&context.conn)
        .unwrap()
        .into_iter()
        .find(|item| item.name == "风景")
        .unwrap();
    let tagged = titles(ArchiveQuery {
        rating_buckets: Some(vec![3, 5]),
        read_statuses: Some(vec!["unread".to_string(), "reading".to_string()]),
        tag_ids: Some(vec![tag.id]),
        tag_mode: Some("and".to_string()),
        collection_id: Some(collection.id),
        sort: Some("title".to_string()),
        direction: Some("asc".to_string()),
        ..ArchiveQuery::default()
    });
    assert_eq!(tagged, vec!["中分", "高分"]);

    let empty = titles(ArchiveQuery {
        rating_buckets: Some(vec![1]),
        read_statuses: Some(vec!["reading".to_string()]),
        ..ArchiveQuery::default()
    });
    assert!(empty.is_empty());

    let grouped = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            rating_buckets: Some(vec![3, 5]),
            group: Some("rating".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(grouped.total, 2);
    assert_eq!(
        grouped
            .groups
            .unwrap()
            .iter()
            .map(|group| (group.bucket, group.count))
            .collect::<Vec<_>>(),
        vec![(5, 1), (3, 1)]
    );
}

#[test]
fn groups_archives_by_rating() {
    let fixture = Fixture::new();

    let high = fixture.source.join("高分.cbz");
    let mid = fixture.source.join("中分.cbz");
    let none = fixture.source.join("未评分.cbz");
    write_zip(&high, &["001.png"], 11);
    write_zip(&mid, &["001.png"], 51);
    write_zip(&none, &["001.png"], 91);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![high, mid, none],
        ImportOptions::default(),
        |_| {},
    );

    let context = db::open_library(&fixture.library).unwrap();
    let all = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(all.total, 3, "errors before grouping");
    let id_of = |keyword: &str| {
        all.items
            .iter()
            .find(|item| item.title.contains(keyword))
            .map(|item| item.id)
            .unwrap()
    };
    db::set_rating(&context.conn, id_of("高分"), 95).unwrap();
    db::set_rating(&context.conn, id_of("中分"), 80).unwrap();
    db::set_rating(&context.conn, id_of("未评分"), 0).unwrap();

    let descending = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            group: Some("rating".to_string()),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(
        descending
            .items
            .iter()
            .map(|item| item.rating)
            .collect::<Vec<_>>(),
        vec![95, 80, 0]
    );
    let groups = descending.groups.expect("grouped page should carry counts");
    assert_eq!(
        groups
            .iter()
            .map(|group| (group.bucket, group.count))
            .collect::<Vec<_>>(),
        vec![(5, 1), (4, 1), (0, 1)]
    );

    let ascending = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            group: Some("rating".to_string()),
            direction: Some("asc".to_string()),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(
        ascending
            .items
            .iter()
            .map(|item| item.rating)
            .collect::<Vec<_>>(),
        vec![0, 80, 95]
    );
    assert_eq!(
        ascending
            .groups
            .unwrap()
            .iter()
            .map(|group| group.bucket)
            .collect::<Vec<_>>(),
        vec![0, 4, 5]
    );

    let plain = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    assert!(plain.groups.is_none());
}

#[test]
fn tag_categories_hierarchy_and_grouping() {
    let fixture = Fixture::new();

    for (name, salt) in [("甲.cbz", 13u8), ("乙.cbz", 47), ("丙.cbz", 83)] {
        write_zip(&fixture.source.join(name), &["001.png"], salt);
    }
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        archive_files(&fixture.source),
        ImportOptions::default(),
        |_| {},
    );

    let mut context = db::open_library(&fixture.library).unwrap();
    let all = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    let id_of = |keyword: &str| {
        all.items
            .iter()
            .find(|item| item.title.contains(keyword))
            .map(|item| item.id)
            .unwrap()
    };

    let tag_a = db::add_tag(&context.conn, id_of("甲"), "A组").unwrap();
    let tag_b = db::add_tag(&context.conn, id_of("甲"), "B组").unwrap();
    db::add_tag(&context.conn, id_of("乙"), "B组").unwrap();

    let root = db::create_tag_category(&context.conn, None, "主题").unwrap();
    assert_eq!(root.path, "主题");
    assert_eq!(root.depth, 0);
    let child = db::create_tag_category(&context.conn, Some("主题"), "画师").unwrap();
    assert_eq!(child.path, "主题/画师");
    assert_eq!(child.parent_id, Some(root.id));
    assert_eq!(child.depth, 1);
    assert!(db::create_tag_category(&context.conn, Some("主题"), "画师").is_err());
    assert!(db::create_tag_category(&context.conn, None, "含/斜杠").is_err());

    let categorized = db::set_tag_category(&context.conn, tag_a.id, Some("主题//画师")).unwrap();
    assert_eq!(categorized.category.as_deref(), Some("主题/画师"));

    let grouped = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            group: Some("tag".to_string()),
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(
        grouped
            .items
            .iter()
            .map(|item| item.title.clone())
            .collect::<Vec<_>>(),
        vec!["甲", "乙", "丙"]
    );
    assert_eq!(
        grouped
            .items
            .iter()
            .map(|item| item.group_bucket)
            .collect::<Vec<_>>(),
        vec![Some(tag_a.id), Some(tag_b.id), Some(0)]
    );
    assert_eq!(grouped.items[0].group_label.as_deref(), Some("A组"));
    assert_eq!(
        grouped.items[0].group_category.as_deref(),
        Some("主题/画师")
    );
    assert_eq!(grouped.items[1].group_label.as_deref(), Some("B组"));
    assert!(grouped.items[1].group_category.is_none());
    assert!(grouped.items[2].group_label.is_none());

    let groups = grouped.groups.expect("tag grouping should carry counts");
    assert_eq!(
        groups
            .iter()
            .map(|group| (group.bucket, group.count, group.label.clone()))
            .collect::<Vec<_>>(),
        vec![
            (tag_a.id, 1, Some("A组".to_string())),
            (tag_b.id, 1, Some("B组".to_string())),
            (0, 1, None),
        ]
    );
    assert_eq!(groups[0].category.as_deref(), Some("主题/画师"));

    let filtered = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            group: Some("tag".to_string()),
            tag_ids: Some(vec![tag_b.id]),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.total, 2);
    assert_eq!(
        filtered
            .items
            .iter()
            .map(|item| item.group_bucket)
            .collect::<Vec<_>>(),
        vec![Some(tag_b.id), Some(tag_b.id)]
    );
    let filtered_groups = filtered.groups.unwrap();
    assert_eq!(filtered_groups.len(), 1);
    assert_eq!(filtered_groups[0].bucket, tag_b.id);
    assert_eq!(filtered_groups[0].count, 2);

    let renamed = db::rename_tag_category(&mut context.conn, "主题/画师", "作者").unwrap();
    assert!(renamed.iter().any(|category| category.path == "主题/作者"));
    let tags = db::list_tags(&context.conn).unwrap();
    let tag_a_row = tags.iter().find(|tag| tag.id == tag_a.id).unwrap();
    assert_eq!(tag_a_row.category.as_deref(), Some("主题/作者"));

    let other = db::create_tag_category(&context.conn, None, "资料").unwrap();
    db::move_tag_category(&mut context.conn, "主题/作者", Some("资料")).unwrap();
    let tags = db::list_tags(&context.conn).unwrap();
    let tag_a_row = tags.iter().find(|tag| tag.id == tag_a.id).unwrap();
    assert_eq!(tag_a_row.category.as_deref(), Some("资料/作者"));
    assert!(db::move_tag_category(&mut context.conn, "资料", Some("资料/作者")).is_err());

    db::move_tag_category(&mut context.conn, "资料/作者", None).unwrap();
    let tags = db::list_tags(&context.conn).unwrap();
    let tag_a_row = tags.iter().find(|tag| tag.id == tag_a.id).unwrap();
    assert_eq!(tag_a_row.category.as_deref(), Some("作者"));

    let remaining = db::delete_tag_category(&mut context.conn, "主题").unwrap();
    assert!(remaining.iter().all(|category| category.path != "主题"));
    assert!(db::delete_tag_category(&mut context.conn, "主题").is_err());

    let cleared = db::set_tag_category(&context.conn, tag_a.id, None).unwrap();
    assert!(cleared.category.is_none());
    assert_eq!(other.path, "资料");
}

#[test]
fn bulk_tag_import_categorizes_and_survives_bad_rows() {
    let fixture = Fixture::new();
    let zip = fixture.source.join("批量标签.cbz");
    write_zip(&zip, &["001.png", "002.png"], 40);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip],
        ImportOptions::default(),
        |_| {},
    );

    let context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let archive_id = page.items[0].id;

    let oversized = "很".repeat(db::MAX_TAG_NAME + 1);
    let report = db::add_tags_bulk(
        &context.conn,
        archive_id,
        &[
            TagDraft {
                name: "长裙".into(),
                category: Some("角色".into()),
            },
            // 批内重复（大小写不同）只算一次
            TagDraft {
                name: "长裙".into(),
                category: Some("其他".into()),
            },
            TagDraft {
                name: "mountainhan".into(),
                category: Some("作者".into()),
            },
            TagDraft {
                name: "全彩".into(),
                category: None,
            },
            // 超长的那条必须只记失败，不拖累其余三条
            TagDraft {
                name: oversized.clone(),
                category: Some("其他".into()),
            },
        ],
    )
    .unwrap();

    assert_eq!(report.added, 3);
    assert_eq!(report.linked, 3);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(
        report.failed[0].reason,
        format!("标签最多 {} 个字符", db::MAX_TAG_NAME)
    );

    let attached = db::archive_tags(&context.conn, archive_id).unwrap();
    assert_eq!(attached.len(), 3);
    let find = |name: &str| attached.iter().find(|tag| tag.name == name).unwrap();
    assert_eq!(find("长裙").category.as_deref(), Some("角色"));
    assert_eq!(find("mountainhan").category.as_deref(), Some("作者"));
    assert_eq!(find("全彩").category, None);

    // 再导一次：不新增、不重复挂、且不覆盖已有分类，只把空的补上。
    let again = db::add_tags_bulk(
        &context.conn,
        archive_id,
        &[
            TagDraft {
                name: "长裙".into(),
                category: Some("其他".into()),
            },
            TagDraft {
                name: "全彩".into(),
                category: Some("其他".into()),
            },
        ],
    )
    .unwrap();

    assert_eq!(again.added, 0);
    assert_eq!(again.existing, 2);
    assert_eq!(again.linked, 0);
    assert_eq!(again.categorized, 1);
    let attached = db::archive_tags(&context.conn, archive_id).unwrap();
    assert_eq!(
        attached
            .iter()
            .find(|tag| tag.name == "长裙")
            .unwrap()
            .category
            .as_deref(),
        Some("角色"),
        "已有分类不能被后续导入覆盖"
    );

    assert!(db::add_tags_bulk(&context.conn, 999_999, &[]).is_err());
}

#[test]
fn tags_and_collections_persist() {
    let fixture = Fixture::new();

    let zip = fixture.source.join("标签测试.cbz");
    write_zip(&zip, &["001.png", "002.png"], 40);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip],
        ImportOptions::default(),
        |_| {},
    );

    let context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    assert_eq!(page.total, 1);
    let archive_id = page.items[0].id;

    let collections = db::list_collections(&context.conn, None).unwrap();
    assert_eq!(collections.len(), 1);
    assert!(collections[0].is_default);
    assert_eq!(collections[0].name, "收藏");
    assert_eq!(collections[0].count, 0);

    let custom = db::create_collection(&context.conn, " 灵感 ").unwrap();
    assert_eq!(custom.name, "灵感");
    assert!(!custom.is_default);
    assert!(db::create_collection(&context.conn, "灵感").is_err());
    assert!(db::create_collection(&context.conn, "   ").is_err());

    db::set_archive_collection(&context.conn, archive_id, custom.id, true).unwrap();
    db::set_archive_collection(&context.conn, archive_id, collections[0].id, true).unwrap();
    db::set_archive_collection(&context.conn, archive_id, custom.id, true).unwrap();
    assert_eq!(
        db::archive_collections(&context.conn, archive_id)
            .unwrap()
            .len(),
        2
    );
    assert!(db::set_archive_collection(&context.conn, archive_id, 999_999, true).is_err());
    assert!(db::set_archive_collection(&context.conn, 999_999, custom.id, true).is_err());

    let filtered = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            collection_id: Some(custom.id),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.total, 1);

    db::set_archive_collection(&context.conn, archive_id, custom.id, false).unwrap();
    let empty = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            collection_id: Some(custom.id),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(empty.total, 0);

    let tag = db::add_tag(&context.conn, archive_id, "  #风景 ").unwrap();
    assert_eq!(tag.name, "风景");
    assert_eq!(tag.count, 1);
    let again = db::add_tag(&context.conn, archive_id, "风景").unwrap();
    assert_eq!(again.id, tag.id);
    assert_eq!(again.count, 1);

    let all_tags = db::list_tags(&context.conn).unwrap();
    assert_eq!(all_tags.len(), 1);
    assert_eq!(all_tags[0].name, "风景");
    assert_eq!(db::archive_tags(&context.conn, archive_id).unwrap().len(), 1);
    assert!(db::add_tag(&context.conn, 999_999, "孤立").is_err());
    assert!(db::add_tag(&context.conn, archive_id, "   ").is_err());

    db::remove_tag(&context.conn, archive_id, tag.id).unwrap();
    assert!(db::archive_tags(&context.conn, archive_id).unwrap().is_empty());
    let retained = db::list_tags(&context.conn).unwrap();
    assert_eq!(retained.len(), 1, "历史标签应保留供后续编辑");
    assert_eq!(retained[0].count, 0);
}

#[test]
fn migrates_v1_tags_to_translated_schema() {
    let fixture = Fixture::new();
    let gallery = fixture.library.join(".gallery");
    // 该用例要造一个 v1 库，先清掉夹具初始化好的 v7 库。
    let _ = fs::remove_dir_all(&gallery);
    fs::create_dir_all(&gallery).unwrap();

    {
        let connection = rusqlite::Connection::open(gallery.join("library.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE archives (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    title TEXT,
                    file_name TEXT,
                    note TEXT
                );
                CREATE TABLE collections (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    is_default INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE tags (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    category TEXT
                );
                CREATE TABLE archive_tags (
                    archive_id INTEGER NOT NULL,
                    tag_id INTEGER NOT NULL,
                    PRIMARY KEY (archive_id, tag_id)
                );
                INSERT INTO tags(name) VALUES ('landscape');
                PRAGMA user_version = 1;",
            )
            .unwrap();
    }

    let context = db::open_library(&fixture.library).unwrap();
    let version: i64 = context
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    // 迁移应当一步推到最新版本（当前 v9，含 tag_text 标签搜索列）。
    assert_eq!(version, db::SCHEMA_VERSION);
    let deleted_column: i64 = context
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('archives') WHERE name = 'deleted_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deleted_column, 1);

    let tags = db::list_tags(&context.conn).unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "landscape");
    assert_eq!(tags[0].original_name, "landscape");
    assert_eq!(tags[0].translated_name.as_deref(), Some("风景"));
}

#[test]
fn tag_center_translation_filter_and_graph() {
    let fixture = Fixture::new();

    let zip_a = fixture.source.join("图 A.cbz");
    let zip_b = fixture.source.join("图 B.cbz");
    write_zip(&zip_a, &["001.png"], 10);
    write_zip(&zip_b, &["001.png"], 70);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip_a, zip_b],
        ImportOptions::default(),
        |_| {},
    );

    let context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    let archive_a = page.items[0].id;
    let archive_b = page.items[1].id;

    let landscape = db::add_tag(&context.conn, archive_a, "Landscape").unwrap();
    assert_eq!(landscape.translated_name.as_deref(), Some("风景"));
    let sunset = db::add_tag(&context.conn, archive_a, "Sunset").unwrap();
    assert_eq!(sunset.translated_name.as_deref(), Some("日落"));
    let landscape_b = db::add_tag(&context.conn, archive_b, "landscape").unwrap();
    assert_eq!(landscape_b.id, landscape.id);
    assert_eq!(landscape_b.count, 2);

    let graph = db::tag_graph(&context.conn, 1, 300).unwrap();
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].weight, 1);

    let filtered = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            tag_ids: Some(vec![landscape.id]),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.total, 2);

    let both = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            tag_ids: Some(vec![landscape.id, sunset.id, landscape.id]),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(both.total, 1);

    let unknown = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            tag_ids: Some(vec![999_999]),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(unknown.total, 0);

    let updated = db::update_tag(
        &context.conn,
        landscape.id,
        "Landscape Photography",
        Some("风景摄影"),
    )
    .unwrap();
    assert_eq!(updated.name, "Landscape Photography");
    assert_eq!(updated.translated_name.as_deref(), Some("风景摄影"));
    assert!(db::update_tag(&context.conn, sunset.id, "landscape photography", None).is_err());
    assert!(db::update_tag(&context.conn, 999_999, "不存在", None).is_err());

    db::remove_tag(&context.conn, archive_a, sunset.id).unwrap();
    let retained = db::list_tags(&context.conn).unwrap();
    let sunset_retained = retained.iter().find(|tag| tag.id == sunset.id).unwrap();
    assert_eq!(sunset_retained.count, 0);

    db::delete_tag(&context.conn, landscape.id).unwrap();
    assert!(db::archive_tags(&context.conn, archive_b).unwrap().is_empty());
    assert!(db::delete_tag(&context.conn, landscape.id).is_err());
}

#[test]
fn search_tag_lifecycle_and_collection_management() {
    let fixture = Fixture::new();

    for (name, salt) in [("城市 夜景.cbz", 5u8), ("海边 日落.cbz", 45), ("森林.cbz", 85)] {
        write_zip(&fixture.source.join(name), &["001.png"], salt);
    }
    let inputs = archive_files(&fixture.source);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        inputs,
        ImportOptions::default(),
        |_| {},
    );

    let mut context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(page.total, 3);
    let id_of = |keyword: &str| {
        page.items
            .iter()
            .find(|item| item.title.contains(keyword))
            .map(|item| item.id)
            .unwrap()
    };
    let city = id_of("城市");
    let sea = id_of("海边");
    let forest = id_of("森林");

    let note = db::set_note(&context.conn, city, Some("雨中的城市夜景")).unwrap();
    assert_eq!(note.as_deref(), Some("雨中的城市夜景"));
    let by_short = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            search: Some("城市".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(by_short.total, 1);
    let by_fts = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            search: Some("城市夜景".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(by_fts.total, 1);
    assert_eq!(by_fts.items[0].note.as_deref(), Some("雨中的城市夜景"));

    let landscape = db::add_tag(&context.conn, city, "Landscape").unwrap();
    let sunset = db::add_tag(&context.conn, sea, "Sunset").unwrap();
    let forest_tag = db::add_tag(&context.conn, forest, "Forest").unwrap();
    assert_eq!(landscape.number, landscape.id);
    assert_eq!(landscape.original_name, "Landscape");

    let any = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            tag_ids: Some(vec![landscape.id, sunset.id]),
            tag_mode: Some("or".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(any.total, 2);

    let excluded = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            tag_ids: Some(vec![landscape.id, sunset.id]),
            tag_mode: Some("exclude".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(excluded.total, 1);
    assert_eq!(excluded.items[0].id, forest);

    let applied = db::set_tag_translations_batch(
        &mut context.conn,
        &[
            TagTranslationInput {
                id: landscape.id,
                translated_name: Some("风景摄影".to_string()),
                source: Some("ai".to_string()),
            },
            TagTranslationInput {
                id: sunset.id,
                translated_name: Some("日落".to_string()),
                source: Some("ai".to_string()),
            },
        ],
    )
    .unwrap();
    assert_eq!(applied, 2);
    let translated = db::list_tags(&context.conn).unwrap();
    let landscape_row = translated.iter().find(|tag| tag.id == landscape.id).unwrap();
    assert_eq!(landscape_row.translated_name.as_deref(), Some("风景摄影"));
    assert_eq!(landscape_row.translation_source.as_deref(), Some("ai"));

    let merged = db::merge_tags(&mut context.conn, &[sunset.id], landscape.id).unwrap();
    assert_eq!(merged.id, landscape.id);
    assert_eq!(merged.count, 2);
    assert!(db::restore_tag(&context.conn, sunset.id).is_err());
    assert!(db::list_deleted_tags(&context.conn)
        .unwrap()
        .iter()
        .any(|tag| tag.id == sunset.id && tag.merged_into == Some(landscape.id)));

    db::delete_tag(&context.conn, forest_tag.id).unwrap();
    assert!(db::archive_tags(&context.conn, forest).unwrap().is_empty());
    assert!(db::list_deleted_tags(&context.conn)
        .unwrap()
        .iter()
        .any(|tag| tag.id == forest_tag.id));
    let restored = db::restore_tag(&context.conn, forest_tag.id).unwrap();
    assert_eq!(restored.id, forest_tag.id);
    assert_eq!(db::archive_tags(&context.conn, forest).unwrap().len(), 1);

    let collections = db::list_collections(&context.conn, Some(&fixture.covers)).unwrap();
    let default_collection = collections.iter().find(|item| item.is_default).unwrap();
    let custom = db::create_collection(&context.conn, "测试收藏").unwrap();
    let renamed = db::rename_collection(&context.conn, custom.id, "重命名收藏").unwrap();
    assert_eq!(renamed.name, "重命名收藏");
    assert!(db::rename_collection(&context.conn, custom.id, "收藏").is_err());

    let added = db::add_archives_to_collection(
        &mut context.conn,
        custom.id,
        &[city, sea, forest, city],
    )
    .unwrap();
    assert_eq!(added, 3);
    let added_again =
        db::add_archives_to_collection(&mut context.conn, custom.id, &[city]).unwrap();
    assert_eq!(added_again, 0);

    let covered = db::set_collection_cover(&context.conn, custom.id, Some(city)).unwrap();
    assert_eq!(covered.cover_archive_id, Some(city));
    let listed = db::list_collections(&context.conn, Some(&fixture.covers)).unwrap();
    assert!(listed
        .iter()
        .find(|item| item.id == custom.id)
        .unwrap()
        .cover_path
        .is_some());
    assert!(db::set_collection_cover(&context.conn, custom.id, Some(999_999)).is_err());

    db::reorder_collections(&mut context.conn, &[custom.id, default_collection.id]).unwrap();
    let ordered = db::list_collections(&context.conn, None).unwrap();
    assert_eq!(ordered[0].id, default_collection.id, "默认收藏夹始终排在最前");

    db::delete_collection(&context.conn, custom.id).unwrap();
    assert!(db::delete_collection(&context.conn, default_collection.id).is_err());
    assert!(db::list_collections(&context.conn, None)
        .unwrap()
        .iter()
        .all(|item| item.id != custom.id));

    let trash = db::list_deleted_collections(&context.conn, None).unwrap();
    let trashed = trash
        .iter()
        .find(|item| item.id == custom.id)
        .expect("deleted collection should be in trash");
    assert_eq!(trashed.count, 3);
    assert!(trashed.deleted_at.is_some());
    assert!(db::create_collection(&context.conn, "重命名收藏").is_err());
    assert!(db::rename_collection(&context.conn, custom.id, "回收站改名").is_err());

    let restored = db::restore_collection(&context.conn, custom.id, None).unwrap();
    assert_eq!(restored.count, 3);
    assert!(restored.deleted_at.is_none());
    assert!(db::restore_collection(&context.conn, custom.id, None).is_err());
    assert!(db::list_collections(&context.conn, None)
        .unwrap()
        .iter()
        .any(|item| item.id == custom.id));

    db::delete_collection(&context.conn, custom.id).unwrap();
    db::purge_collection(&context.conn, custom.id).unwrap();
    assert!(db::purge_collection(&context.conn, custom.id).is_err());
    assert!(db::restore_collection(&context.conn, custom.id, None).is_err());
    assert!(db::purge_collection(&context.conn, default_collection.id).is_err());
    let recreated = db::create_collection(&context.conn, "重命名收藏").unwrap();
    assert_ne!(recreated.id, custom.id);
}

#[test]
fn group_archives_by_collection() {
    let fixture = Fixture::new();
    for (name, salt) in [("甲.cbz", 17u8), ("乙.cbz", 29), ("丙.cbz", 41), ("丁.cbz", 53)] {
        write_zip(&fixture.source.join(name), &["001.png"], salt);
    }
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        archive_files(&fixture.source),
        ImportOptions::default(),
        |_| {},
    );

    let context = db::open_library(&fixture.library).unwrap();
    let all = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    let id_of = |title: &str| {
        all.items
            .iter()
            .find(|item| item.title == title)
            .unwrap()
            .id
    };

    let default_collection = db::list_collections(&context.conn, None).unwrap();
    let default_collection = default_collection
        .iter()
        .find(|collection| collection.is_default)
        .unwrap();
    let featured = db::create_collection(&context.conn, "精选").unwrap();
    let later = db::create_collection(&context.conn, "稍后").unwrap();

    db::set_archive_collection(&context.conn, id_of("甲"), featured.id, true).unwrap();
    db::set_archive_collection(&context.conn, id_of("甲"), later.id, true).unwrap();
    db::set_archive_collection(&context.conn, id_of("乙"), default_collection.id, true).unwrap();
    db::set_archive_collection(&context.conn, id_of("丙"), later.id, true).unwrap();

    let grouped = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            group: Some("collection".to_string()),
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(
        grouped
            .items
            .iter()
            .map(|item| item.title.clone())
            .collect::<Vec<_>>(),
        vec!["乙", "甲", "丙", "丁"]
    );
    assert_eq!(
        grouped
            .items
            .iter()
            .map(|item| item.group_bucket)
            .collect::<Vec<_>>(),
        vec![
            Some(default_collection.id),
            Some(featured.id),
            Some(later.id),
            Some(0)
        ]
    );
    assert_eq!(grouped.items[0].group_label.as_deref(), Some("收藏"));
    assert_eq!(grouped.items[1].group_label.as_deref(), Some("精选"));
    assert_eq!(grouped.items[2].group_label.as_deref(), Some("稍后"));
    assert!(grouped.items[3].group_label.is_none());
    assert!(grouped.items[1].group_category.is_none());

    let groups = grouped.groups.expect("collection grouping should carry counts");
    assert_eq!(
        groups
            .iter()
            .map(|group| (group.bucket, group.count, group.label.clone()))
            .collect::<Vec<_>>(),
        vec![
            (default_collection.id, 1, Some("收藏".to_string())),
            (featured.id, 1, Some("精选".to_string())),
            (later.id, 1, Some("稍后".to_string())),
            (0, 1, None),
        ]
    );

    let filtered = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            group: Some("collection".to_string()),
            collection_id: Some(later.id),
            page_size: Some(12),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.total, 2);
    assert_eq!(
        filtered
            .items
            .iter()
            .map(|item| item.group_bucket)
            .collect::<Vec<_>>(),
        vec![Some(later.id), Some(later.id)]
    );
    let filtered_groups = filtered.groups.unwrap();
    assert_eq!(filtered_groups.len(), 1);
    assert_eq!(filtered_groups[0].bucket, later.id);
    assert_eq!(filtered_groups[0].count, 2);
}

#[test]
fn recent_reading_sort_keeps_unread_last() {
    let fixture = Fixture::new();
    for (name, salt) in [("甲.cbz", 11u8), ("乙.cbz", 22), ("丙.cbz", 33)] {
        write_zip(&fixture.source.join(name), &["01.png", "02.png"], salt);
    }

    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        archive_files(&fixture.source),
        ImportOptions::default(),
        |_| {},
    );
    assert_eq!(report.imported, 3, "errors: {:?}", report.errors);

    let context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("title".to_string()),
            direction: Some("asc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    let id_of = |title: &str| {
        page.items
            .iter()
            .find(|item| item.title == title)
            .unwrap()
            .id
    };
    let (jia, yi, bing) = (id_of("甲"), id_of("乙"), id_of("丙"));
    assert!(page.items.iter().all(|item| item.last_read_at.is_none()));

    context
        .conn
        .execute(
            "UPDATE archives SET last_read_at = 1000, last_page = 1 WHERE id = ?1",
            [jia],
        )
        .unwrap();
    context
        .conn
        .execute(
            "UPDATE archives SET last_read_at = 2000, last_page = 2 WHERE id = ?1",
            [yi],
        )
        .unwrap();

    let titles = |query: ArchiveQuery| {
        library::query_archives(&context.conn, &fixture.library, &fixture.covers, &query)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.title)
            .collect::<Vec<_>>()
    };

    let recent = titles(ArchiveQuery {
        sort: Some("last_read_at".to_string()),
        direction: Some("desc".to_string()),
        ..ArchiveQuery::default()
    });
    assert_eq!(recent, ["乙", "甲", "丙"]);

    let oldest = titles(ArchiveQuery {
        sort: Some("last_read_at".to_string()),
        direction: Some("asc".to_string()),
        ..ArchiveQuery::default()
    });
    assert_eq!(oldest, ["甲", "乙", "丙"]);

    let cards = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery {
            sort: Some("last_read_at".to_string()),
            direction: Some("desc".to_string()),
            ..ArchiveQuery::default()
        },
    )
    .unwrap();
    assert_eq!(cards.items[0].last_page, 2);
    assert_eq!(cards.items[0].last_read_at, Some(2000));
    assert!(cards.items[2].last_read_at.is_none());
    assert_eq!(cards.items[2].last_page, 0);
    assert!(bing > 0);
}

#[test]
fn reader_reads_pages_and_persists_progress() {
    let fixture = Fixture::new();
    let zip = fixture.source.join("Reader.cbz");
    write_zip(
        &zip,
        &["10.png", "2.png", "page1.png", ".hidden.png", "notes.txt"],
        30,
    );

    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip],
        ImportOptions::default(),
        |_| {},
    );
    assert_eq!(report.imported, 1, "errors: {:?}", report.errors);

    let context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let archive_id = page.items[0].id;

    let source = db::archive_source(&context.conn, archive_id).unwrap();
    let path = fixture.library.join(&source.rel_path);

    let info = crate::reader::reader_info(&path, 0).unwrap();
    assert_eq!(info.page_count, 3);
    assert_eq!(info.last_page, 0);

    // 正在显示的那一页传 pin=true：预取邻页时不能被挤掉。
    let first = crate::reader::read_page(&path, 0, true).unwrap();
    assert_eq!(first.name, "2.png");
    assert!(!first.version.is_empty(), "版本号用于前端缓存 URL");
    assert!(first.bytes > 0);

    // 页面字节留在后端缓存里，前端只按 URL 取图，不再走 base64 IPC。
    let payload = crate::reader::cached_page(&path, 0).expect("读过的页应已预热");
    assert_eq!(payload.mime, "image/png");
    assert!(!payload.bytes.is_empty());
    assert!(
        crate::reader::cached_page(&path, 2).is_none(),
        "没读过的页不应出现在缓存里"
    );

    let last = crate::reader::read_page(&path, 2, false).unwrap();
    assert_eq!(last.name, "page1.png");
    assert!(crate::reader::read_page(&path, 3, false).is_err());
    assert!(crate::reader::read_page(&path, -1, false).is_err());

    let thumb = crate::reader::read_thumbnail(&path, 0, Some(96)).unwrap();
    assert_eq!(thumb.name, "2.png");
    assert!(thumb.data_url.starts_with("data:image/jpeg;base64,"));
    let fallback = crate::reader::read_thumbnail(&path, 1, None).unwrap();
    assert!(fallback.data_url.starts_with("data:image/jpeg;base64,"));
    assert!(crate::reader::read_thumbnail(&path, 9, Some(96)).is_err());

    db::set_reading_progress(&context.conn, archive_id, 2).unwrap();
    let source = db::archive_source(&context.conn, archive_id).unwrap();
    assert_eq!(source.last_page, 2);
    assert!(source.missing == false);

    let refreshed = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let card = refreshed
        .items
        .iter()
        .find(|item| item.id == archive_id)
        .unwrap();
    assert_eq!(card.last_page, 2);
    assert!(card.last_read_at.is_some());
    let resumed = crate::reader::reader_info(&path, source.last_page).unwrap();
    assert_eq!(resumed.last_page, 2);

    let over = crate::reader::reader_info(&path, 99).unwrap();
    assert_eq!(over.last_page, 3);

    let bookmark = db::add_reader_bookmark(&context.conn, archive_id, 2).unwrap();
    assert_eq!(bookmark.page, 2);
    let repeated = db::add_reader_bookmark(&context.conn, archive_id, 2).unwrap();
    assert_eq!(repeated.id, bookmark.id);
    let bookmarks = db::reader_bookmarks(&context.conn, archive_id).unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].page, 2);
    assert!(db::add_reader_bookmark(&context.conn, archive_id, 0).is_err());
    assert!(db::add_reader_bookmark(&context.conn, archive_id, 4).is_err());

    db::remove_reader_bookmark(&context.conn, archive_id, bookmark.id).unwrap();
    assert!(db::reader_bookmarks(&context.conn, archive_id)
        .unwrap()
        .is_empty());
    assert!(db::remove_reader_bookmark(&context.conn, archive_id, bookmark.id).is_err());
}

#[test]
fn archive_trash_lifecycle() {
    let fixture = Fixture::new();
    for (name, salt) in [("甲.cbz", 71u8), ("乙.cbz", 83), ("丙.cbz", 97)] {
        write_zip(&fixture.source.join(name), &["001.png", "002.png"], salt);
    }
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        archive_files(&fixture.source),
        ImportOptions::default(),
        |_| {},
    );

    let context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let id_of = |title: &str| {
        page.items
            .iter()
            .find(|item| item.title == title)
            .unwrap()
            .id
    };
    let target = id_of("甲");
    let keep = id_of("乙");

    let collection = db::create_collection(&context.conn, "回收测试").unwrap();
    for id in [target, keep, id_of("丙")] {
        db::set_archive_collection(&context.conn, id, collection.id, true).unwrap();
    }
    let tag = db::add_tag(&context.conn, target, "回收站标签").unwrap();

    db::delete_archive(&context.conn, target).unwrap();
    assert!(db::delete_archive(&context.conn, target).is_err());

    let visible = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    assert_eq!(visible.total, 2);
    assert!(visible.items.iter().all(|item| item.id != target));

    let (count, _, _) = db::stats(&context.conn).unwrap();
    assert_eq!(count, 2);
    let collections = db::list_collections(&context.conn, Some(&fixture.covers)).unwrap();
    let trash_collection = collections
        .iter()
        .find(|item| item.id == collection.id)
        .unwrap();
    assert_eq!(trash_collection.count, 2);
    let tags = db::list_tags(&context.conn).unwrap();
    let trash_tag = tags.iter().find(|item| item.id == tag.id).unwrap();
    assert_eq!(trash_tag.count, 0);

    let trashed = db::list_deleted_archives(&context.conn, Some(&fixture.covers)).unwrap();
    assert_eq!(trashed.len(), 1);
    assert_eq!(trashed[0].id, target);
    assert_eq!(trashed[0].title, "甲");
    assert_eq!(trashed[0].page_count, 2);
    assert!(trashed[0].deleted_at > 0);
    assert!(db::set_rating(&context.conn, target, 90).is_err());
    assert!(db::add_tag(&context.conn, target, "不允许").is_err());
    assert!(db::set_reading_progress(&context.conn, target, 1).is_err());

    db::restore_archive(&context.conn, target).unwrap();
    assert!(db::restore_archive(&context.conn, target).is_err());
    let restored = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    assert_eq!(restored.total, 3);
    let collections = db::list_collections(&context.conn, Some(&fixture.covers)).unwrap();
    let trash_collection = collections
        .iter()
        .find(|item| item.id == collection.id)
        .unwrap();
    assert_eq!(trash_collection.count, 3);
    let tags = db::list_tags(&context.conn).unwrap();
    let trash_tag = tags.iter().find(|item| item.id == tag.id).unwrap();
    assert_eq!(trash_tag.count, 1);
    assert!(db::list_deleted_archives(&context.conn, None).unwrap().is_empty());

    db::delete_archive(&context.conn, target).unwrap();
    assert!(db::purge_archive(&context.conn, keep).is_err());
    db::purge_archive(&context.conn, target).unwrap();
    assert!(db::list_deleted_archives(&context.conn, None).unwrap().is_empty());
    let final_page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    assert_eq!(final_page.total, 2);
    let links: i64 = context
        .conn
        .query_row(
            "SELECT COUNT(*) FROM collection_archives WHERE archive_id = ?1",
            [target],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(links, 0);
    let tag_links: i64 = context
        .conn
        .query_row(
            "SELECT COUNT(*) FROM archive_tags WHERE archive_id = ?1",
            [target],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tag_links, 0);
}

#[test]
fn scan_marks_missing_in_one_batch_and_is_idempotent() {
    let fixture = Fixture::new();
    let zip_a = fixture.source.join("A.cbz");
    let zip_b = fixture.source.join("B.cbz");
    write_zip(&zip_a, &["001.png"], 3);
    write_zip(&zip_b, &["001.png"], 4);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip_a, zip_b],
        ImportOptions::default(),
        |_| {},
    );

    let mut files = archive_files(&fixture.library);
    files.sort();
    assert_eq!(files.len(), 2);
    fs::remove_file(&files[0]).unwrap();

    let first = library::scan_with_covers(&fixture.library, &fixture.covers);
    assert_eq!(
        first.missing, 1,
        "第一次扫描应标记 1 个缺失：{}",
        first.message
    );

    // 已经标记过的不应重复计数（批量标记语句带 missing = 0 条件）
    let second = library::scan_with_covers(&fixture.library, &fixture.covers);
    assert_eq!(
        second.missing, 0,
        "第二次扫描不应重复标记：{}",
        second.message
    );
}

#[test]
fn worker_connection_requires_initialized_library() {
    let fixture = Fixture::new();
    fs::remove_dir_all(fixture.library.join(".gallery")).unwrap();

    // 导入/扫描只开工作连接，不再顺带建库：未初始化的目录必须明确报错，
    // 而不是在任意路径下悄悄生成 .gallery/library.db。
    let error = db::open_worker_connection(&fixture.library).unwrap_err();
    assert!(
        error.contains("已初始化的画廊库"),
        "意外的错误信息：{error}"
    );
}

#[test]
fn library_json_with_unsafe_id_is_regenerated() {
    let fixture = Fixture::new();
    let path = fixture.library.join(".gallery").join("library.json");
    let raw = fs::read_to_string(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    // library_id 会被拼进封面缓存目录，含 .. 时可以写出缓存目录之外
    value["library_id"] = serde_json::json!("../../escape");
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    let context = db::open_library(&fixture.library).unwrap();
    assert!(
        !context.library_id.contains("..") && !context.library_id.contains('/'),
        "library_id 未被清洗：{}",
        context.library_id
    );
    assert!(
        context
            .library_id
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == '-')
    );
}

#[test]
fn migration_creates_indexes_for_the_sort_dropdown() {
    // 「页数」「最近阅读」两个排序键原来没有索引，深翻页要整表排序后再 OFFSET。
    let fixture = Fixture::new();
    let context = db::open_library(&fixture.library).unwrap();
    let indexed: i64 = context
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index'
               AND name IN ('idx_archives_page_count', 'idx_archives_last_read_at')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 2);
}

#[test]
fn archive_query_rejects_unknown_field_typos() {
    // 字段名拼错（pageSize -> pagesize）必须报错，而不是静默取默认值后
    // 表现成「筛选没生效」。
    let error = serde_json::from_str::<ArchiveQuery>(r#"{"pagesize": 24}"#).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
    assert!(serde_json::from_str::<ArchiveQuery>(r#"{"pageSize": 24}"#).is_ok());
}

#[test]
fn concurrent_writes_wait_for_the_lock_instead_of_failing() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    let holder_path = fixture.library.clone();
    let (locked_tx, locked_rx) = mpsc::channel();

    // 另一条连接先拿走写锁并持有 700ms。
    // 用 channel 握手而不是 sleep 固定时长：机器负载高时（并行跑测试、后台在编译）
    // 占锁线程可能 120ms 还没跑到 INSERT，主线程就会立刻拿到锁、误判「没有等待」，
    // 所以改成「占锁线程确认拿到锁之后再放主线程去抢」。
    let holder = std::thread::spawn(move || {
        let connection = db::open_worker_connection(&holder_path).unwrap();
        let transaction = db::begin_write(&connection).unwrap();
        transaction
            .execute(
                "INSERT INTO collections(name, is_default, created_at, sort_order)
                 VALUES('占用中的收藏', 0, 0, 99)",
                [],
            )
            .unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(700));
        transaction.commit().unwrap();
    });

    locked_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("占锁线程没能及时拿到写锁");
    let other = db::open_worker_connection(&fixture.library).unwrap();
    let started = Instant::now();
    let transaction =
        db::begin_write(&other).expect("应当等待写锁，而不是立刻报 database is locked");
    let waited = started.elapsed();
    transaction.rollback().unwrap();
    holder.join().unwrap();

    assert!(
        waited >= Duration::from_millis(300),
        "没有发生等待，说明写锁没有被真正持有过：{waited:?}"
    );
}

#[test]
#[ignore = "性能基线，需手动运行：cargo test --lib -- --ignored bench_open_and_deep_paging --nocapture"]
fn bench_open_and_deep_paging() {
    use std::time::Instant;

    let fixture = Fixture::new();
    let context = db::open_library(&fixture.library).unwrap();

    let transaction = db::begin_write(&context.conn).unwrap();
    for index in 0..10_000i64 {
        transaction
            .execute(
                "INSERT INTO archives
                   (rel_path, file_name, ext, file_size, file_mtime, page_count, title, rating, added_at)
                 VALUES (?1, ?2, 'zip', 4096, 0, 12, ?3, 60, ?4)",
                rusqlite::params![
                    format!("dir/{index}.zip"),
                    format!("画集 {index}.zip"),
                    format!("标题 {index}"),
                    index
                ],
            )
            .unwrap();
    }
    transaction.commit().unwrap();

    let started = Instant::now();
    let reopened = db::open_library(&fixture.library).unwrap();
    println!(
        "open_library（含迁移检查 + 全文索引完整性检查，1 万条）: {:?}",
        started.elapsed()
    );

    let started = Instant::now();
    let worker = db::open_worker_connection(&fixture.library).unwrap();
    println!(
        "open_worker_connection（仅连接 + PRAGMA）: {:?}",
        started.elapsed()
    );
    drop(worker);

    let first = ArchiveQuery {
        page: Some(1),
        page_size: Some(48),
        ..Default::default()
    };
    let started = Instant::now();
    let page =
        library::query_archives(&reopened.conn, &fixture.library, &fixture.covers, &first).unwrap();
    println!(
        "第 1 页：{} 条 / total={}，{:?}",
        page.items.len(),
        page.total,
        started.elapsed()
    );

    let deep = ArchiveQuery {
        page: Some(200),
        page_size: Some(48),
        ..Default::default()
    };
    let started = Instant::now();
    let page =
        library::query_archives(&reopened.conn, &fixture.library, &fixture.covers, &deep).unwrap();
    println!(
        "第 200 页（OFFSET 9552）：{} 条，{:?}",
        page.items.len(),
        started.elapsed()
    );
}

#[test]
fn import_stage_preview_and_tags_on_import() {
    let fixture = Fixture::new();
    let zip = fixture.source.join("A 画集.cbz");
    write_zip(&zip, &["cover.png", "page2.png", "page3.png"], 12);

    // 暂存：拖进来的是目录，应展开成 1 本；页数走 zip 中央目录，不解码封面。
    let (staged, errors) = importer::gather_inputs(
        &fixture.library,
        std::slice::from_ref(&fixture.source),
    );
    assert!(errors.is_empty(), "errors: {errors:?}");
    assert_eq!(staged.len(), 1);
    assert!(!staged[0].in_library, "源目录在库外");
    assert!(staged[0].issue.is_none());
    assert_eq!(crate::archive::count_pages(&zip).unwrap(), 3);

    // 导入时直接把标签挂上，分类不存在就建出来。
    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip.clone()],
        ImportOptions {
            add_to_collection: None,
            preserve_structure: None,
            tags: vec![
                TagDraft {
                    name: "胡桃".into(),
                    category: Some("角色/原神".into()),
                },
                TagDraft {
                    name: "全彩".into(),
                    category: Some("其他".into()),
                },
            ],
        },
        |_| {},
    );
    assert_eq!(report.imported, 1, "errors: {:?}", report.errors);
    assert!(report.errors.is_empty(), "tags: {:?}", report.errors);

    let context = db::open_library(&fixture.library).unwrap();
    let page =
        library::query_archives(&context.conn, &fixture.library, &fixture.covers, &ArchiveQuery::default())
            .unwrap();
    let archive_id = page.items[0].id;

    let attached = db::archive_tags(&context.conn, archive_id).unwrap();
    assert_eq!(attached.len(), 2, "标签应随导入一起挂上");
    let find = |name: &str| attached.iter().find(|tag| tag.name == name).unwrap();
    assert!(
        find("胡桃")
            .category
            .as_deref()
            .unwrap_or_default()
            .starts_with("角色"),
        "显式路径应落进角色大类"
    );
    assert_eq!(find("全彩").category.as_deref(), Some("其他"));

    // 「疑似重复」只看同名同大小，命中后导入流程自己还会按内容哈希复核。
    let size = fs::metadata(&zip).unwrap().len() as i64;
    assert!(db::has_similar_archive(&context.conn, "A 画集.cbz", size).unwrap());
    assert!(!db::has_similar_archive(&context.conn, "A 画集.cbz", size + 1).unwrap());

    // 再导一次同一本：不复制、不重复建记录，但面板里新填的标签要补到已有记录上。
    let again = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip],
        ImportOptions {
            tags: vec![TagDraft {
                name: "补的标签".into(),
                category: None,
            }],
            ..Default::default()
        },
        |_| {},
    );
    assert_eq!(again.imported, 0);
    assert_eq!(again.skipped, 1, "errors: {:?}", again.errors);
    let attached = db::archive_tags(&context.conn, archive_id).unwrap();
    assert!(
        attached.iter().any(|tag| tag.name == "补的标签"),
        "重复导入时标签应补到已有画集上"
    );
}

/// v8 迁移必须给老库补上 read_count / tags.weight 与两张待看目录表。
#[test]
fn migration_v8_adds_read_count_and_queues() {
    let fixture = Fixture::new();
    let context = db::open_library(&fixture.library).unwrap();

    for (table, column) in [("archives", "read_count"), ("tags", "weight")] {
        let count: i64 = context
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "{table}.{column} 未建出来");
    }

    let tables: i64 = context
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN ('queues', 'queue_archives')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 2, "待看目录表未建出来");

    let version: i64 = context
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, db::SCHEMA_VERSION);
}

/// 权重为 0 的标签必须把整本画集排除出候选，而不是只把它的权重算 0。
#[test]
fn zero_weight_tag_excludes_archive_entirely() {
    let fixture = Fixture::new();

    let keep = fixture.source.join("保留的本.cbz");
    let drop = fixture.source.join("排除的本.cbz");
    write_zip(&keep, &["a.png"], 11);
    write_zip(&drop, &["b.png"], 22);

    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![keep, drop],
        ImportOptions {
            tags: vec![TagDraft {
                name: "共同标签".into(),
                category: None,
            }],
            ..Default::default()
        },
        |_| {},
    );
    assert_eq!(report.imported, 2, "errors: {:?}", report.errors);

    let mut context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let drop_id = page
        .items
        .iter()
        .find(|item| item.title == "排除的本")
        .unwrap()
        .id;

    db::add_tag(&context.conn, drop_id, "不想看的标签").unwrap();
    let tags = db::list_tags(&context.conn).unwrap();
    let zero_tag = tags.iter().find(|t| t.name == "不想看的标签").unwrap();
    assert_eq!(zero_tag.weight, db::DEFAULT_TAG_WEIGHT, "新标签默认 100 权");
    db::set_tag_weight(&context.conn, zero_tag.id, 0).unwrap();

    let fill = crate::queue::fill_queue(
        &mut context.conn,
        "测试目录",
        &crate::models::QueueFillOptions {
            size: Some(10),
            tag_ids: vec![],
            include_read: None,
        },
        Some(1),
    )
    .unwrap();

    assert_eq!(fill.added, 1, "应只挑到 1 本：{:?}", fill.titles);
    assert_eq!(fill.titles, vec!["保留的本".to_string()]);
    assert_eq!(fill.excluded_by_weight, 1, "被 0 权标签排除 1 本");
}

/// 看得少的更容易被挑中：连跑多个种子，新本应显著多于已看过很多遍的。
#[test]
fn queue_prefers_less_read_archives() {
    let fixture = Fixture::new();

    let fresh = fixture.source.join("新本.cbz");
    let seen = fixture.source.join("看过的本.cbz");
    write_zip(&fresh, &["a.png"], 31);
    write_zip(&seen, &["b.png"], 44);

    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![fresh, seen],
        ImportOptions::default(),
        |_| {},
    );
    assert_eq!(report.imported, 2, "errors: {:?}", report.errors);

    let mut context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let seen_id = page
        .items
        .iter()
        .find(|item| item.title == "看过的本")
        .unwrap()
        .id;

    for _ in 0..9 {
        db::record_read(&context.conn, seen_id).unwrap();
    }
    let reread = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let counter = reread
        .items
        .iter()
        .find(|item| item.title == "看过的本")
        .unwrap();
    assert_eq!(counter.read_count, 9, "阅读次数应累计");

    let mut fresh_hits = 0;
    let mut seen_hits = 0;
    for seed in 0..120u64 {
        // 每次换目录名，避免「已在本目录里」把候选清空
        let name = format!("抽样-{seed}");
        let fill = crate::queue::fill_queue(
            &mut context.conn,
            &name,
            &crate::models::QueueFillOptions {
                size: Some(1),
                tag_ids: vec![],
                include_read: Some(true),
            },
            Some(seed),
        )
        .unwrap();
        match fill.titles.first().map(String::as_str) {
            Some("新本") => fresh_hits += 1,
            Some("看过的本") => seen_hits += 1,
            other => panic!("意外结果：{other:?}"),
        }
    }
    assert!(
        fresh_hits > seen_hits,
        "新本应更容易被挑中：fresh={fresh_hits} seen={seen_hits}"
    );
}

/// 待看目录最多 3 个，超出的淘汰最旧的。
#[test]
fn queues_are_capped_at_three() {
    let fixture = Fixture::new();
    let zip = fixture.source.join("A.cbz");
    write_zip(&zip, &["a.png"], 5);

    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip],
        ImportOptions::default(),
        |_| {},
    );
    assert_eq!(report.imported, 1, "errors: {:?}", report.errors);

    let mut context = db::open_library(&fixture.library).unwrap();
    for index in 0..5 {
        crate::queue::fill_queue(
            &mut context.conn,
            &format!("目录-{index}"),
            &crate::models::QueueFillOptions {
                size: Some(1),
                tag_ids: vec![],
                include_read: Some(true),
            },
            Some(index),
        )
        .unwrap();
    }

    let queues = crate::queue::list_queues(&context.conn).unwrap();
    assert_eq!(queues.len(), 3, "待看目录最多保留 3 个：{queues:?}");
    assert_eq!(queues[0].name, "目录-4", "最新建的在最前");
}

/// 待看目录进度：读完的本计入 finished。
#[test]
fn queue_progress_counts_finished() {
    let fixture = Fixture::new();
    let a = fixture.source.join("A.cbz");
    let b = fixture.source.join("B.cbz");
    write_zip(&a, &["1.png", "2.png"], 61);
    write_zip(&b, &["1.png"], 62);

    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![a, b],
        ImportOptions::default(),
        |_| {},
    );
    assert_eq!(report.imported, 2, "errors: {:?}", report.errors);

    let mut context = db::open_library(&fixture.library).unwrap();
    crate::queue::fill_queue(
        &mut context.conn,
        "进度测试",
        &crate::models::QueueFillOptions {
            size: Some(10),
            tag_ids: vec![],
            include_read: Some(true),
        },
        Some(9),
    )
    .unwrap();

    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let a_id = page
        .items
        .iter()
        .find(|item| item.title == "A")
        .unwrap()
        .id;
    // A 有两页，标到第 2 页 = 读完
    db::set_reading_progress(&context.conn, a_id, 2).unwrap();

    let queues = crate::queue::list_queues(&context.conn).unwrap();
    assert_eq!(queues[0].count, 2);
    assert_eq!(queues[0].finished, 1, "A 读完应计 1 本");
}

/// 回收站里的画集不再累计阅读次数。
#[test]
fn record_read_rejects_deleted_archive() {
    let fixture = Fixture::new();
    let zip = fixture.source.join("A.cbz");
    write_zip(&zip, &["a.png"], 71);
    let report = importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        vec![zip],
        ImportOptions::default(),
        |_| {},
    );
    assert_eq!(report.imported, 1, "errors: {:?}", report.errors);

    let context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let id = page.items[0].id;

    db::record_read(&context.conn, id).unwrap();
    db::delete_archive(&context.conn, id).unwrap();
    assert!(
        db::record_read(&context.conn, id).is_err(),
        "已进回收站的画集不应再计数"
    );
}

#[test]
fn daily_backup_skips_fresh_and_prunes_old() {
    let fixture = Fixture::new();
    let backups = fixture.library.join(".gallery").join("backups");

    // Fixture::new 里的 open_library 已经写了一份日备；确认它存在。
    let fresh: Vec<_> = fs::read_dir(&backups)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("library-daily-"))
        })
        .collect();
    assert_eq!(fresh.len(), 1, "首次打开应写一份日备");

    // 20 小时内再次打开：跳过，不新增。
    let context = db::open_library(&fixture.library).unwrap();
    drop(context);
    let after_skip = fs::read_dir(&backups).unwrap().count();
    assert_eq!(after_skip, 1, "20 小时内重复打开不应新增日备");

    // 伪造 3 份过期日备（时间戳远小于当前，排序在最前）。
    for index in 0..3i64 {
        let stamp: i64 = 1_000_000_000_000 + index; // 2001 年，肯定过期
        fs::write(backups.join(format!("library-daily-{stamp}.db")), b"stale").unwrap();
    }
    let context = db::open_library(&fixture.library).unwrap();
    drop(context);

    let mut names: Vec<String> = fs::read_dir(&backups)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("library-daily-"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 3, "日备应滚动保留最近 3 份");
    assert!(
        !names.iter().any(|name| name.contains("1000000000000")),
        "最老的过期日备应被清理"
    );
}

#[test]
fn search_matches_tag_translated_names() {
    let fixture = Fixture::new();

    for (name, salt) in [("城市 夜景.cbz", 5u8), ("海边 日落.cbz", 45)] {
        write_zip(&fixture.source.join(name), &["001.png"], salt);
    }
    let inputs = archive_files(&fixture.source);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        inputs,
        ImportOptions::default(),
        |_| {},
    );

    let mut context = db::open_library(&fixture.library).unwrap();
    let all = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    assert_eq!(all.total, 2);
    let city = all
        .items
        .iter()
        .find(|item| item.title.contains("城市"))
        .map(|item| item.id)
        .unwrap();
    let sea = all
        .items
        .iter()
        .find(|item| item.title.contains("海边"))
        .map(|item| item.id)
        .unwrap();

    let landscape = db::add_tag(&context.conn, city, "Landscape").unwrap();
    let sunset = db::add_tag(&context.conn, sea, "Sunset").unwrap();
    db::set_tag_translation(&context.conn, landscape.id, Some("风景摄影"), "ai").unwrap();
    db::set_tag_translation(&context.conn, sunset.id, Some("日落"), "ai").unwrap();

    let search = |keyword: &str| {
        library::query_archives(
            &context.conn,
            &fixture.library,
            &fixture.covers,
            &ArchiveQuery {
                search: Some(keyword.to_string()),
                ..ArchiveQuery::default()
            },
        )
        .unwrap()
        .total
    };

    // ≥3 字走 FTS：原文名与中文译名都要命中（英文 9 字符、中文 4 字）。
    assert_eq!(search("Landscape"), 1, "FTS 应命中标签原文名");
    assert_eq!(search("风景摄影"), 1, "FTS 应命中中文译名");
    assert_eq!(search("Sunset"), 1);
    // 2 字走 LIKE 兜底：trigram 打不中的短中文译名也要命中。
    assert_eq!(search("日落"), 1, "2 字译名应由 LIKE 兜底命中");
    assert_eq!(search("风景"), 1, "2 字译名前缀也应命中");

    // 摘掉标签后不再命中，且重挂后恢复（触发器双向维护）。
    db::remove_tag(&context.conn, city, landscape.id).unwrap();
    assert_eq!(search("风景摄影"), 0, "摘除标签后 tag_text 应清空");
    assert_eq!(search("日落"), 1, "另一本不受影响");
    db::add_tag(&context.conn, city, "Landscape").unwrap();
    assert_eq!(search("风景摄影"), 1, "重挂标签后译名搜索应恢复");
}

#[test]
fn queue_manual_add_and_remove() {
    use crate::fsutil;
    use crate::queue;

    let fixture = Fixture::new();
    write_zip(&fixture.source.join("A.cbz"), &["a.png"], 7u8);
    let inputs = archive_files(&fixture.source);
    importer::import_with_progress(
        &fixture.library,
        &fixture.covers,
        inputs,
        ImportOptions::default(),
        |_| {},
    );

    let mut context = db::open_library(&fixture.library).unwrap();
    let page = library::query_archives(
        &context.conn,
        &fixture.library,
        &fixture.covers,
        &ArchiveQuery::default(),
    )
    .unwrap();
    let archive_id = page.items[0].id;

    context
        .conn
        .execute(
            "INSERT INTO queues(name, created_at) VALUES ('手动目录', ?1)",
            [fsutil::now_ms()],
        )
        .unwrap();
    let queue_id = context.conn.last_insert_rowid();

    // 正常加入 → 重复加入幂等返回 false
    assert!(queue::add_to_queue(&context.conn, queue_id, archive_id).unwrap());
    assert!(!queue::add_to_queue(&context.conn, queue_id, archive_id).unwrap());
    assert_eq!(
        queue::queue_archives(&context.conn, queue_id).unwrap(),
        vec![archive_id]
    );

    // 目录不存在 / 画集不存在（用不存在的 id 冒充）都要报错
    assert!(queue::add_to_queue(&context.conn, 9999, archive_id).is_err());
    assert!(queue::add_to_queue(&context.conn, queue_id, 424242).is_err());

    // 回收站里的画集不能加入
    db::delete_archive(&context.conn, archive_id).unwrap();
    assert!(queue::add_to_queue(&context.conn, queue_id, archive_id).is_err());

    // 恢复后可以移出；重复移出幂等
    db::restore_archive(&context.conn, archive_id).unwrap();
    queue::remove_from_queue(&context.conn, queue_id, archive_id).unwrap();
    assert!(queue::queue_archives(&context.conn, queue_id).unwrap().is_empty());
    queue::remove_from_queue(&context.conn, queue_id, archive_id).unwrap();
}
