use axum::{routing::{get, post}, Json, Router, extract::State};
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use firestore::*;
use dotenvy::dotenv;
use std::env;
use teloxide::prelude::*;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use axum::extract::Path;

// --- МОДЕЛИ ДАННЫХ ---

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Transaction {
    sender: String,
    receiver: String,
    amount: f64,
    payload: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Block {
    index: u32,
    timestamp: i64,
    transactions: Vec<Transaction>,
    prev_hash: String,
    hash: String,
    validator: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct MewWallet {
    address: String,
    public_key: String,
    secret_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WalletStats {
    address: String,
    balance: f64,
    apy_earned: f64,
    tasks_completed: u32,
}

struct AppState {
    db: FirestoreDb,
    bot: Bot,
    chat_id: String,
}

// --- Wallet Functions ---
impl MewWallet {
    // 1. Создание кошелька
    fn create_new() -> Self {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = VerifyingKey::from(&signing_key);

        let pub_hex = hex::encode(verifying_key.as_bytes());
        let sec_hex = hex::encode(signing_key.to_bytes());
        
        MewWallet {
            address: format!("mew013{}", &pub_hex[..24]), // Твой формат адреса
            public_key: pub_hex,
            secret_key: sec_hex,
        }
    }

    // 2. Импорт по секретному ключу
    fn import_from_secret(secret_hex: &str) -> Result<Self, String> {
        let secret_bytes = hex::decode(secret_hex).map_err(|_| "Invalid hex")?;
        let bytes: [u8; 32] = secret_bytes.try_into().map_err(|_| "Invalid length")?;
        
        let signing_key = SigningKey::from_bytes(&bytes);
        let verifying_key = VerifyingKey::from(&signing_key);

        let pub_hex = hex::encode(verifying_key.as_bytes());

        Ok(MewWallet {
            address: format!("mew013{}", &pub_hex[..24]),
            public_key: pub_hex,
            secret_key: secret_hex.to_string(),
        })
    }
}

async fn add_block(
    State(state): State<Arc<AppState>>,
    Json(new_block): Json<Block>,
) -> Json<String> {
    // 1. Сохраняем в Firestore (используем индекс блока как имя документа)
    let _ : Block = state.db.fluent()
        .insert()
        .into("blocks")
        .document_id(new_block.index.to_string())
        .object(&new_block)
        .execute()
        .await
        .expect("Failed to write block to Firestore");

    // 2. Формируем отчет для Telegram
    let report = format!(
        "📦 *Новый блок #{}*\n Hash: `{}`\n Валидатор: `{}`\n Транзакций: {}",
        new_block.index, new_block.hash, new_block.validator, new_block.transactions.len()
    );

    // 3. Отправляем в ТГ
    let _ = state.bot
        .send_message(state.chat_id.clone(), report)
        .await;

    Json(format!("Блок #{} успешно добавлен в сеть", new_block.index))
}

async fn get_blocks(State(state): State<Arc<AppState>>) -> Json<Vec<Block>> {
    let blocks: Vec<Block> = state.db.fluent()
        .select()
        .from("blocks")
        .order_by([("index", FirestoreQueryOrder::Ascending)])
        .obj()
        .query()
        .await
        .unwrap_or_default();

    Json(blocks)
}

// Эндпоинт для создания кошелька
async fn create_wallet(State(state): State<Arc<AppState>>) -> Json<MewWallet> {
    let wallet = MewWallet::create_new();
    
    // Сразу создаем запись в Firestore для этого кошелька
    let initial_stats = WalletStats {
        address: wallet.address.clone(),
        balance: 0.0,
        apy_earned: 0.0,
        tasks_completed: 0,
    };

    // Записываем в коллекцию "wallets", используя адрес как ID документа
    let _ : WalletStats = state.db.fluent()
        .insert()
        .into("wallets")
        .document_id(&wallet.address)
        .object(&initial_stats)
        .execute()
        .await
        .expect("Failed to register wallet in Firestore");

    Json(wallet)
}

// Эндпоинт для получения реальной статистики
async fn get_wallet_stats(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Json<WalletStats> {
    // Ищем документ в коллекции "wallets" по адресу (ID документа)
    let stats: Option<WalletStats> = state.db.fluent()
        .select()
        .by_id_in("wallets")
        .obj()
        .one(&address)
        .await
        .unwrap_or(None);

    // Если нашли — отдаем данные, если нет — создаем пустую структуру
    Json(stats.unwrap_or(WalletStats {
        address,
        balance: 0.0,
        apy_earned: 0.0,
        tasks_completed: 0,
    }))
}

// Структура для принятия ключа из JSON
#[derive(Deserialize)]
struct ImportRequest {
    secret_key: String,
}

async fn import_wallet(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ImportRequest>,
) -> Result<Json<MewWallet>, (axum::http::StatusCode, String)> {
    // 1. Пытаемся восстановить кошелек из ключа
    let wallet = MewWallet::import_from_secret(&payload.secret_key)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;

    // 2. Проверяем, есть ли он в базе, если нет — создаем начальные статы
    let _: WalletStats = state.db.fluent()
        .insert()
        .into("wallets")
        .document_id(&wallet.address)
        .object(&WalletStats {
            address: wallet.address.clone(),
            balance: 0.0,
            apy_earned: 0.0,
            tasks_completed: 0,
        })
        .execute()
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB Error".to_string()))?;

    Ok(Json(wallet))
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let project_id = env::var("FIREBASE_PROJECT_ID").expect("FIREBASE_PROJECT_ID не задан");
    let bot_token = env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN не задан");
    let chat_id = env::var("TELEGRAM_CHAT_ID").expect("TELEGRAM_CHAT_ID не задан");
    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());

    // Инициализация Firestore
    let db = FirestoreDb::with_options(FirestoreDbOptions::new(project_id))
        .await
        .expect("Error Firebase connect");

    // Инициализация Telegram Бота
    let bot = Bot::new(bot_token);

    // Создаем общее состояние
    let shared_state = Arc::new(AppState {
        db,
        bot,
        chat_id,
    });

    // Настройка роутера
    let app = Router::new()
        .route("/blocks", get(get_blocks))
        .route("/add_block", post(add_block))
        .route("/wallet/new", get(create_wallet)) // Создать новый
        .route("/wallet/import", post(import_wallet))
        .route("/wallet/:address", get(get_wallet_stats)) // Получить статы
        .with_state(shared_state);
        
    // Запуск сервера
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("🚀 MewChain Core запущена на {}", addr);

    axum::serve(listener, app).await.unwrap();
}
