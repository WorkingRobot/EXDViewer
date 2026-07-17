use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::utils::{HttpResponse, request, yield_to_ui};

const API: &str = "https://api.github.com";

#[derive(Debug, Clone)]
pub struct PrDraft {
    /// e.g. xivdev
    pub base_owner: String,
    /// e.g. EXDSchema
    pub base_repo: String,
    /// e.g. latest
    pub base_branch: String,
    pub title: String,
    pub body: String,
    // (path, content)
    pub files: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct PrResult {
    pub html_url: String,
    pub number: u64,
}

pub struct GithubClient {
    token: String,
}

impl GithubClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    async fn call(&self, method: &str, url: &str, body: Option<&Value>) -> Result<HttpResponse> {
        let auth = format!("Bearer {}", self.token);
        let mut headers = vec![
            ("Authorization", auth.as_str()),
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
            // Ignored by browsers but required on native
            ("User-Agent", "EXDViewer"),
        ];
        let body_bytes = match body {
            Some(value) => {
                headers.push(("Content-Type", "application/json"));
                Some(serde_json::to_vec(value)?)
            }
            None => None,
        };
        request(method, url, &headers, body_bytes).await
    }

    async fn call_json(&self, method: &str, url: &str, body: Option<&Value>) -> Result<Value> {
        let resp = self.call(method, url, body).await?;
        let json: Value = if resp.bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&resp.bytes).unwrap_or(Value::Null)
        };

        if !(200..300).contains(&resp.status) {
            let message = json
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| resp.text());
            bail!(
                "GitHub API {method} {url} failed ({}): {message}",
                resp.status
            );
        }

        Ok(json)
    }

    pub async fn current_login(&self) -> Result<String> {
        let user = self.call_json("GET", &format!("{API}/user"), None).await?;
        str_field(&user, "login").map(str::to_owned)
    }

    pub async fn submit_pr(&self, draft: &PrDraft) -> Result<PrResult> {
        if draft.files.is_empty() {
            bail!("No changed schemas to submit");
        }

        let PrDraft {
            base_owner,
            base_repo,
            base_branch,
            ..
        } = draft;

        let login = self.current_login().await?;

        // Create/ensure fork
        self.call_json(
            "POST",
            &format!("{API}/repos/{base_owner}/{base_repo}/forks"),
            Some(&json!({})),
        )
        .await?;
        let fork = format!("{API}/repos/{login}/{base_repo}");
        self.wait_for_fork(&fork).await?;

        // Resolve base commit and tree
        let base_ref = self
            .call_json(
                "GET",
                &format!("{API}/repos/{base_owner}/{base_repo}/git/ref/heads/{base_branch}"),
                None,
            )
            .await?;
        let base_sha = nested_str(&base_ref, &["object", "sha"])?.to_owned();
        let base_commit = self
            .call_json(
                "GET",
                &format!("{API}/repos/{base_owner}/{base_repo}/git/commits/{base_sha}"),
                None,
            )
            .await?;
        let base_tree = nested_str(&base_commit, &["tree", "sha"])?.to_owned();

        // Create blob per file, create tree from blobs
        let mut tree_entries = Vec::with_capacity(draft.files.len());
        for (path, content) in &draft.files {
            let blob = self
                .call_json(
                    "POST",
                    &format!("{fork}/git/blobs"),
                    Some(&json!({ "content": content, "encoding": "utf-8" })),
                )
                .await?;
            let blob_sha = str_field(&blob, "sha")?;
            tree_entries.push(json!({
                "path": path,
                "mode": "100644",
                "type": "blob",
                "sha": blob_sha,
            }));
        }
        let tree = self
            .call_json(
                "POST",
                &format!("{fork}/git/trees"),
                Some(&json!({ "base_tree": base_tree, "tree": tree_entries })),
            )
            .await?;
        let tree_sha = str_field(&tree, "sha")?.to_owned();

        // Commit tree, point branch at commit
        let commit = self
            .call_json(
                "POST",
                &format!("{fork}/git/commits"),
                Some(&json!({
                    "message": draft.title,
                    "tree": tree_sha,
                    "parents": [base_sha],
                })),
            )
            .await?;
        let commit_sha = str_field(&commit, "sha")?.to_owned();

        let branch = self
            .create_branch(
                &fork,
                &generate_branch_name(&base_sha, &draft.files),
                &commit_sha,
            )
            .await?;

        // Open PR
        let pr = self
            .call_json(
                "POST",
                &format!("{API}/repos/{base_owner}/{base_repo}/pulls"),
                Some(&json!({
                    "title": draft.title,
                    "body": draft.body,
                    "head": format!("{login}:{branch}"),
                    "base": base_branch,
                })),
            )
            .await?;

        Ok(PrResult {
            html_url: str_field(&pr, "html_url")?.to_owned(),
            number: pr
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("PR response missing 'number'"))?,
        })
    }

    // Returns name of branch created
    async fn create_branch(&self, fork: &str, base_name: &str, commit_sha: &str) -> Result<String> {
        for attempt in 0..10u32 {
            let name = if attempt == 0 {
                base_name.to_owned()
            } else {
                format!("{base_name}-{}", attempt + 1)
            };
            let resp = self
                .call(
                    "POST",
                    &format!("{fork}/git/refs"),
                    Some(&json!({ "ref": format!("refs/heads/{name}"), "sha": commit_sha })),
                )
                .await?;
            if resp.ok {
                return Ok(name);
            }
            // 422 = ref already exists (name taken)
            if resp.status != 422 {
                let json: Value = serde_json::from_slice(&resp.bytes).unwrap_or(Value::Null);
                let message = json
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| resp.text());
                bail!("Failed to create branch ({}): {message}", resp.status);
            }
        }
        bail!("Could not find an unused branch name")
    }

    async fn wait_for_fork(&self, fork_url: &str) -> Result<()> {
        const MAX_ATTEMPTS: u32 = 15;
        for attempt in 0..MAX_ATTEMPTS {
            let resp = self.call("GET", fork_url, None).await?;
            if resp.ok {
                return Ok(());
            }
            if resp.status != 404 {
                bail!("Failed to access fork ({}): {}", resp.status, resp.text());
            }
            log::debug!("Waiting for fork to become available (attempt {attempt})");
            yield_to_ui().await;
        }
        bail!("Fork did not become available in time; please try again")
    }
}

// files are (name, content)
fn generate_branch_name(base_sha: &str, files: &[(String, String)]) -> String {
    let stem = files
        .first()
        .map(|(path, _)| {
            path.rsplit('/')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".yml")
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "schemas".to_owned());
    let label = if files.len() > 1 {
        format!("{}-schemas", files.len())
    } else {
        stem
    };

    let mut hasher = Sha256::new();
    hasher.update(base_sha.as_bytes());
    for (path, content) in files {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(content.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let suffix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();

    format!("exdviewer/{label}-{suffix}")
}

fn str_field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("GitHub response missing string field '{key}'"))
}

fn nested_str<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
    let mut current = value;
    for key in path {
        current = current
            .get(key)
            .ok_or_else(|| anyhow!("GitHub response missing field '{key}'"))?;
    }
    current
        .as_str()
        .ok_or_else(|| anyhow!("GitHub response field '{}' is not a string", path.join(".")))
}
