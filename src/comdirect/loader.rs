use crate::comdirect::session_client::PersistentSession;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct SessionLoader {
    path: String,
}

impl SessionLoader {
    pub fn new(path: String) -> Self {
        SessionLoader { path }
    }

    pub async fn load_session(&self) -> Option<PersistentSession> {
        load_session_from_file(&self.path).await.ok()
    }

    pub async fn save_session(&self, session: &PersistentSession) -> Result<(), Box<FileError>> {
        save_session_to_file(session, &self.path).await
    }

    pub async fn clear_session(&self) {
        delete_session_file(&self.path).await;
    }
}

#[derive(Debug)]
pub enum FileError {
    SerializeError,
    OpenError,
    WriteError,
    ReadError,
}

async fn save_session_to_file(
    session: &PersistentSession,
    path: &str,
) -> Result<(), Box<FileError>> {
    let json = serde_json::to_string(&session).map_err(|_| FileError::SerializeError)?;

    let mut file = File::create(path).await.map_err(|_| FileError::OpenError)?;

    file.write_all(json.as_bytes())
        .await
        .map_err(|_| FileError::WriteError)?;

    Ok(())
}

async fn load_session_from_file(path: &str) -> Result<PersistentSession, Box<FileError>> {
    let mut file = File::open(path).await.map_err(|_| FileError::ReadError)?;
    let mut content = String::new();

    file.read_to_string(&mut content)
        .await
        .map_err(|_| FileError::ReadError)?;

    let session: PersistentSession =
        serde_json::from_reader(content.as_bytes()).map_err(|_| FileError::ReadError)?;

    Ok(session)
}

async fn delete_session_file(path: &str) {
    match tokio::fs::remove_file(path).await {
        Ok(_) => {
            println!("Session file deleted successfully.");
        }
        Err(e) => {
            eprintln!("Failed to delete session file: {}", e);
        }
    }
}

#[cfg(test)]
mod test {
    use crate::comdirect::loader::SessionLoader;
    use crate::comdirect::session_client::PersistentSession;
    use std::path::Path;
    use tokio::fs;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_save_load_session() {
        let unique_id = Uuid::new_v4().to_string();
        let test_file_path = format!("/tmp/test_session_{}.json", unique_id);
        let path_obj = Path::new(&test_file_path);

        let loader = SessionLoader::new(test_file_path.clone());

        let original_session = PersistentSession {
            access_token: "test_access_token".to_string(),
            refresh_token: "test_refresh_token".to_string(),
            session_uuid: "test_session_id".to_string(),
        };

        let save_result = loader.save_session(&original_session).await;

        assert!(
            save_result.is_ok(),
            "Failed to save session: {:?}",
            save_result.err()
        );
        assert!(path_obj.exists(), "Session file was not created");

        let loaded_session_option = loader.load_session().await;
        assert!(loaded_session_option.is_some(), "Failed to load session");

        let loaded_session = loaded_session_option.unwrap();

        assert_eq!(
            original_session, loaded_session,
            "Loaded session does not match original session"
        );

        if path_obj.exists() {
            let _ = fs::remove_file(&test_file_path);
        }
    }
}
