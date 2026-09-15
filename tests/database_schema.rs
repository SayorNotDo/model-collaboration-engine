//! Unsupported schemas must be rejected without altering existing data.
use model_collaboration_engine::store::{SqliteStore, Store, SCHEMA_VERSION};

#[tokio::test]
async fn fresh_database_initializes_current_schema_and_reopens() {
    let directory = tempfile::tempdir().unwrap();
    for precreate in [false, true] {
        let path = directory.path().join(format!("fresh-{precreate}.db"));
        if precreate {
            std::fs::File::create(&path).unwrap();
        }
        let store = SqliteStore::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(store.metrics().await.unwrap().tasks.total, 0);
        store.close().await.unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION,
        );
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master
             WHERE type='table' AND name IN ('tasks','attempts','events','evaluations','feedback')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 5);
        drop(connection);
        let reopened = SqliteStore::open(path.to_str().unwrap()).await.unwrap();
        assert!(reopened.metrics().await.unwrap().quality.is_empty());
        reopened.close().await.unwrap();
    }
}

#[tokio::test]
async fn unsupported_schemas_preserve_file_and_unresolved_ledger() {
    let directory = tempfile::tempdir().unwrap();
    for version in [0, 1, 99] {
        let path = directory.path().join(format!("unsupported-{version}.db"));
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!("planning/legacy-v1.sql"))
            .unwrap();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        connection
            .execute(
                "INSERT INTO tasks(id,spec,config_hash,plan,status,total,reserved,calls,checkpoint)
             VALUES('old','{}','old','{}','cancelled',100,7,1,'{}')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO attempts(id,task,amount,state,metadata)
             VALUES('attempt','old',7,'unresolved','{}')",
                [],
            )
            .unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        // Repeat to check that rejection also releases database ownership.
        for _ in 0..2 {
            let error = match SqliteStore::open(path.to_str().unwrap()).await {
                Ok(_) => panic!("unsupported schema accepted"),
                Err(error) => error,
            };
            assert_eq!(error.kind, "schema");
            assert_eq!(error.details["current"], version);
            assert_eq!(error.details["target"], SCHEMA_VERSION);
            assert_eq!(error.details["action"], "backup_and_rebuild");
            assert!(error.message.contains("new database_path"));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let connection = rusqlite::Connection::open(&path).unwrap();
        let reserved: i64 = connection
            .query_row("SELECT reserved FROM tasks WHERE id='old'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(reserved, 7);
    }
}

#[tokio::test]
async fn unrelated_database_is_never_reinitialized() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("foreign.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE personal_data(value TEXT); INSERT INTO personal_data VALUES('keep');",
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        SqliteStore::open(path.to_str().unwrap()).await,
        Err(error) if error.kind == "schema" && error.message.contains("not an engine database")
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
