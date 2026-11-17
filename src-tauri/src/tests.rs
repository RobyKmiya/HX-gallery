use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use image::{ImageFormat, Rgb, RgbImage};
use uuid::Uuid;
use zip::write::SimpleFileOptions;

use crate::db;
use crate::importer;
use crate::library;
use crate::models::{ArchiveQuery, ImportOptions, TagTranslationInput};

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
    assert_eq!(version, 7);
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

    let first = crate::reader::read_page(&path, 0).unwrap();
    assert_eq!(first.name, "2.png");
    assert!(first.data_url.starts_with("data:image/png;base64,"));

    let last = crate::reader::read_page(&path, 2).unwrap();
    assert_eq!(last.name, "page1.png");
    assert!(crate::reader::read_page(&path, 3).is_err());
    assert!(crate::reader::read_page(&path, -1).is_err());

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
