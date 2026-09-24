//! Project grouping (spec § 18).

use chrono::Utc;
use uuid::Uuid;

use crate::error::Result;
use crate::Store;

/// A project row grouping assets and jobs.
#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
}

impl Store {
    /// Create a project, returning its id.
    pub fn create_project(&self, name: &str, description: Option<&str>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.conn().execute(
            r#"
            INSERT INTO projects (id, name, description, created_at) VALUES (?1, ?2, ?3, ?4)
            "#,
            rusqlite::params![id, name, description, Utc::now().timestamp_millis()],
        )?;
        Ok(id)
    }

    /// Find a project by id.
    pub fn project(&self, id: &str) -> Result<Option<Project>> {
        use rusqlite::OptionalExtension;
        self.conn()
            .query_row(
                "SELECT id, name, description, created_at FROM projects WHERE id = ?1",
                [id],
                |row| {
                    Ok(Project {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// List projects.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, description, created_at FROM projects ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Count jobs associated with a project.
    pub fn project_job_count(&self, project_id: &str) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM jobs WHERE project_id = ?1",
            [project_id],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_lifecycle() {
        let store = Store::in_memory().unwrap();
        let id = store
            .create_project("Client A", Some("delivery batch"))
            .unwrap();
        let p = store.project(&id).unwrap().unwrap();
        assert_eq!(p.name, "Client A");
        assert_eq!(store.projects().unwrap().len(), 1);
        assert_eq!(store.project_job_count(&id).unwrap(), 0);
    }
}
