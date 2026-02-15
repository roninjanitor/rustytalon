//! Main setup wizard orchestration.
//!
//! The wizard guides users through:
//! 1. Database connection
//! 2. Security (secrets master key)
//! 3. LLM Provider configuration (API keys)
//! 4. Model selection
//! 5. Embeddings
//! 6. Channel configuration
//! 7. Heartbeat (background tasks)

#[cfg(feature = "postgres")]
use deadpool_postgres::{Config as PoolConfig, Runtime};
#[cfg(feature = "postgres")]
use tokio_postgres::NoTls;

use crate::settings::{KeySource, Settings};
use crate::setup::prompts::{
    confirm, input, optional_input, print_header, print_info, print_step,
    print_success, select_one,
};

/// Setup wizard error.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Channel setup error: {0}")]
    Channel(String),

    #[error("User cancelled")]
    Cancelled,
}

/// Setup wizard configuration.
#[derive(Debug, Clone, Default)]
pub struct SetupConfig {
    /// Skip authentication step.
    pub skip_auth: bool,
    /// Only reconfigure channels.
    pub channels_only: bool,
}

/// Interactive setup wizard for RustyTalon.
pub struct SetupWizard {
    config: SetupConfig,
    settings: Settings,
}

impl SetupWizard {
    /// Create a new setup wizard.
    pub fn new() -> Self {
        Self {
            config: SetupConfig::default(),
            settings: Settings::default(),
        }
    }

    /// Create a wizard with custom configuration.
    pub fn with_config(config: SetupConfig) -> Self {
        Self {
            config,
            settings: Settings::default(),
        }
    }

    /// Run the setup wizard interactively.
    pub async fn run(&mut self) -> Result<(), SetupError> {
        print_header("RustyTalon Setup");
        println!();
        println!("Welcome to RustyTalon - a multi-provider AI assistant.");
        println!("This wizard will help you configure the essential settings.");
        println!();

        if self.config.channels_only {
            // Only configure channels
            self.step_channels().await?;
        } else {
            // Full setup
            self.step_database().await?;
            self.step_security().await?;

            if !self.config.skip_auth {
                self.step_provider_config()?;
            }

            self.step_model_selection()?;
            self.step_embeddings()?;
            self.step_channels().await?;
            self.step_heartbeat()?;
        }

        // Save settings
        self.save_settings().await?;

        print_header("Setup Complete");
        println!();
        print_success("RustyTalon is ready to use!");
        println!();
        println!("Start the agent with: rustytalon");
        println!();

        Ok(())
    }

    /// Step 1: Database configuration.
    async fn step_database(&mut self) -> Result<(), SetupError> {
        print_step(1, 7, "Database Configuration");
        println!();

        // Check if DATABASE_URL is already set
        if let Ok(url) = std::env::var("DATABASE_URL") {
            print_info("Found DATABASE_URL in environment");

            if confirm("Use this database connection?", true).map_err(SetupError::Io)? {
                return self.test_database_connection(&url).await;
            }
        }

        // Ask for database URL
        let url = input("Enter PostgreSQL connection URL").map_err(SetupError::Io)?;

        // Test connection
        self.test_database_connection(&url).await?;

        // Save to ~/.rustytalon/.env
        crate::bootstrap::save_database_url(&url)
            .map_err(|e| SetupError::Database(format!("Failed to save DATABASE_URL: {}", e)))?;

        print_success("Database URL saved to ~/.rustytalon/.env");

        Ok(())
    }

    #[cfg(feature = "postgres")]
    async fn test_database_connection(&mut self, url: &str) -> Result<(), SetupError> {
        print_info("Testing database connection...");

        let mut cfg = PoolConfig::new();
        cfg.url = Some(url.to_string());

        let pool = cfg
            .create_pool(Some(Runtime::Tokio1), NoTls)
            .map_err(|e| SetupError::Database(format!("Failed to create pool: {}", e)))?;

        let client = pool
            .get()
            .await
            .map_err(|e| SetupError::Database(format!("Failed to connect: {}", e)))?;

        // Test query
        client
            .execute("SELECT 1", &[])
            .await
            .map_err(|e| SetupError::Database(format!("Query failed: {}", e)))?;

        print_success("Database connection successful");

        Ok(())
    }

    #[cfg(not(feature = "postgres"))]
    async fn test_database_connection(&mut self, _url: &str) -> Result<(), SetupError> {
        print_info("PostgreSQL support not compiled in, skipping connection test");
        Ok(())
    }

    /// Step 2: Security configuration.
    async fn step_security(&mut self) -> Result<(), SetupError> {
        print_step(2, 7, "Security Configuration");
        println!();

        print_info("RustyTalon encrypts sensitive data (API keys, tokens) at rest.");
        println!();

        let options = [
            "System keychain (recommended)",
            "Environment variable",
            "Skip (disable encryption)",
        ];

        let choice = select_one("Where should the encryption key be stored?", &options)
            .map_err(SetupError::Io)?;

        let key_source = match choice {
            0 => KeySource::Keychain,
            1 => KeySource::Env,
            _ => KeySource::None,
        };

        self.settings.secrets_master_key_source = key_source;
        print_success(&format!("Key storage: {:?}", self.settings.secrets_master_key_source));

        Ok(())
    }

    /// Step 3: LLM Provider configuration.
    fn step_provider_config(&mut self) -> Result<(), SetupError> {
        print_step(3, 7, "LLM Provider Configuration");
        println!();

        print_info("RustyTalon supports multiple LLM providers.");
        print_info("You'll need an API key from at least one provider.");
        println!();

        let providers = [
            ("Anthropic (Claude)", "ANTHROPIC_API_KEY"),
            ("OpenAI (GPT-4)", "OPENAI_API_KEY"),
            ("Ollama (Local)", "No API key needed"),
        ];

        let options: Vec<&str> = providers.iter().map(|(name, _)| *name).collect();
        let choice = select_one("Select your primary provider:", &options)
            .map_err(SetupError::Io)?;

        match choice {
            0 => {
                // Anthropic
                if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                    print_info("Found ANTHROPIC_API_KEY in environment");
                } else {
                    print_info("Set ANTHROPIC_API_KEY environment variable with your API key");
                    print_info("Get your key at: https://console.anthropic.com/");
                }
            }
            1 => {
                // OpenAI
                if std::env::var("OPENAI_API_KEY").is_ok() {
                    print_info("Found OPENAI_API_KEY in environment");
                } else {
                    print_info("Set OPENAI_API_KEY environment variable with your API key");
                    print_info("Get your key at: https://platform.openai.com/api-keys");
                }
            }
            2 => {
                // Ollama
                print_info("Make sure Ollama is running locally (ollama serve)");
            }
            _ => {}
        }

        print_success(&format!("Provider: {}", options[choice]));
        Ok(())
    }

    /// Step 4: Model selection.
    fn step_model_selection(&mut self) -> Result<(), SetupError> {
        print_step(4, 7, "Model Selection");
        println!();

        // Default to Anthropic models
        let models: Vec<(&str, &str)> = vec![
            ("claude-sonnet-4-20250514", "Claude Sonnet 4 (recommended)"),
            ("claude-3-5-sonnet-20241022", "Claude 3.5 Sonnet"),
            ("claude-3-5-haiku-20241022", "Claude 3.5 Haiku (faster, cheaper)"),
            ("gpt-4o", "GPT-4o"),
            ("gpt-4o-mini", "GPT-4o Mini"),
            ("llama3", "Llama 3 (Ollama)"),
        ];

        let options: Vec<&str> = models.iter().map(|(_, desc)| *desc).collect();
        let mut all_options = options.clone();
        all_options.push("Custom model ID");

        let choice = select_one("Select a model:", &all_options).map_err(SetupError::Io)?;

        let selected_model = if choice == all_options.len() - 1 {
            input("Enter model ID").map_err(SetupError::Io)?
        } else {
            models[choice].0.to_string()
        };

        self.settings.selected_model = Some(selected_model.clone());
        print_success(&format!("Selected: {}", selected_model));

        Ok(())
    }

    /// Step 5: Embeddings configuration.
    fn step_embeddings(&mut self) -> Result<(), SetupError> {
        print_step(5, 7, "Embeddings Configuration");
        println!();

        print_info("Embeddings enable semantic search in your workspace memory.");
        println!();

        if !confirm("Enable semantic search?", true).map_err(SetupError::Io)? {
            self.settings.embeddings.enabled = false;
            print_info("Embeddings disabled");
            return Ok(());
        }

        self.settings.embeddings.enabled = true;
        self.settings.embeddings.provider = "openai".to_string();
        self.settings.embeddings.model = "text-embedding-3-small".to_string();

        if std::env::var("OPENAI_API_KEY").is_err() {
            print_info("Note: Embeddings require OPENAI_API_KEY to be set");
        }

        print_success("Embeddings enabled with OpenAI text-embedding-3-small");

        Ok(())
    }

    /// Step 6: Channel configuration.
    async fn step_channels(&mut self) -> Result<(), SetupError> {
        print_step(6, 7, "Channel Configuration");
        println!();

        print_info("Channels let you interact with RustyTalon through different interfaces.");
        println!();

        // CLI is always enabled
        print_success("CLI: Always enabled");

        // Ask about HTTP API
        if confirm("Enable HTTP API?", false).map_err(SetupError::Io)? {
            self.settings.channels.http_enabled = true;
            let port = optional_input("HTTP port", Some("3000")).map_err(SetupError::Io)?;
            self.settings.channels.http_port = port.and_then(|p| p.parse().ok());
            print_success(&format!("HTTP API enabled on port {}",
                self.settings.channels.http_port.unwrap_or(3000)));
        }

        // Ask about Telegram
        if confirm("Configure Telegram bot?", false).map_err(SetupError::Io)? {
            print_info("Set TELEGRAM_BOT_TOKEN environment variable with your bot token");
            print_info("Get a token from @BotFather on Telegram");
        }

        Ok(())
    }

    /// Step 7: Heartbeat configuration.
    fn step_heartbeat(&mut self) -> Result<(), SetupError> {
        print_step(7, 7, "Background Tasks");
        println!();

        print_info("Heartbeat enables scheduled routines and background processing.");
        println!();

        if confirm("Enable background tasks?", true).map_err(SetupError::Io)? {
            self.settings.heartbeat.enabled = true;
            print_success("Background tasks enabled");
        } else {
            self.settings.heartbeat.enabled = false;
            print_info("Background tasks disabled");
        }

        Ok(())
    }

    /// Print configuration summary.
    ///
    /// In Docker mode, all configuration comes from environment variables.
    /// This just prints a summary of what the user needs to set.
    async fn save_settings(&mut self) -> Result<(), SetupError> {
        println!();
        print_header("Configuration Summary");
        println!();
        print_info("Set these environment variables in your Docker container or .env file:");
        println!();
        println!("  DATABASE_URL=<your postgres connection string>");

        if let Some(ref model) = self.settings.selected_model {
            println!("  ANTHROPIC_MODEL={} (or OPENAI_MODEL for OpenAI)", model);
        }

        println!("  ANTHROPIC_API_KEY=<your key> (or OPENAI_API_KEY for OpenAI)");

        if self.settings.embeddings.enabled {
            println!("  OPENAI_API_KEY=<your key> (for embeddings)");
        }

        if self.settings.channels.http_enabled {
            println!("  HTTP_ENABLED=true");
            if let Some(port) = self.settings.channels.http_port {
                println!("  HTTP_PORT={}", port);
            }
        }

        println!();

        Ok(())
    }
}

impl Default for SetupWizard {
    fn default() -> Self {
        Self::new()
    }
}
