use axum::{routing::{get, post}, Json, Router, extract::State};
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use firestore::*;
use dotenvy::dotenv;
use std::env;
use teloxide::prelude::*;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use tower_http::services::ServeDir;

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
    last_claim: i64,
}

struct AppState {
    db: FirestoreDb,
    bot: Bot,
    chat_id: String,
}

#[derive(Deserialize)]
struct SendRequest {
    sender_address: String,
    receiver_address: String,
    amount: f64,
}

// Структура для принятия ключа из JSON
#[derive(Deserialize)]
struct ImportRequest {
    secret_key: String,
}

#[derive(Deserialize)]
struct CompleteTaskRequest {
    address: String,
    task_id: String, // Поле с подчеркиванием
    reward: f64,      // Сумма награды
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct GroupMember {
    address: String,
    joined_at: i64,      // Unix timestamp вступления
    last_ping: i64,      // Последний подтвержденный час активности
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct NodeGroup {
    id: String,          // Уникальный ID или имя группы
    owner: String,       // Адрес создателя
    members: Vec<GroupMember>,
    total_mined: f64,    // Сколько всего группа добыла за всё время
}

#[derive(Deserialize)]
struct JoinRequest {
    address: String,
    group_id: String,
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
    let _: () = state.db.fluent()
        .insert()
        .into("blocks")
        .document_id(new_block.index.to_string())
        .object(&new_block)
        .execute::<()>()
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
        .order_by([("index", firestore::FirestoreQueryDirection::Ascending)]) // Если Asc не сработал, верни Ascending, но проверь скобки
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
        last_claim: 0,
    };

    // Записываем в коллекцию "wallets", используя адрес как ID документа
    let _: () = state.db.fluent()
        .insert()
        .into("wallets")
        .document_id(&wallet.address)
        .object(&initial_stats)
        .execute::<()>()
        .await
        .expect("Failed to register wallet in Firestore");

    Json(wallet)
}

// Эндпоинт для получения реальной статистики
async fn get_wallet_stats(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Json<WalletStats> {
    let stats_opt: Option<WalletStats> = state.db.fluent()
        .select()
        .by_id_in("wallets")
        .obj()
        .one(&address)
        .await
        .unwrap_or(None);

    match stats_opt {
        Some(mut stats) => {
            let now = chrono::Utc::now().timestamp();
            
            // Если last_claim > 0 (кошелек активен), считаем APY
            if stats.last_claim > 0 && now > stats.last_claim {
                let seconds_passed = now - stats.last_claim;
                
                // 7% годовых в секунду
                let apy_per_second = 0.07 / (365.0 * 24.0 * 3600.0);
                let reward = stats.balance * apy_per_second * (seconds_passed as f64);
                
                if reward > 0.00000001 { // Не мучаем базу из-за микро-сумм
                    stats.balance += reward;
                    stats.apy_earned += reward;
                    stats.last_claim = now; // Обновляем метку времени

                    // Сохраняем обновленный баланс в фоне
                    let db_clone = state.db.clone();
                    let stats_clone = stats.clone();
                    tokio::spawn(async move {
                        let _ = db_clone.fluent()
                            .update()
                            .fields(paths!(WalletStats::{balance, apy_earned, last_claim}))
                            .in_col("wallets")
                            .document_id(&stats_clone.address)
                            .object(&stats_clone)
                            .execute::<()>()
                            .await;
                    });
                }
            }
            Json(stats)
        }
        None => Json(WalletStats {
            address,
            balance: 0.0,
            apy_earned: 0.0,
            tasks_completed: 0,
            last_claim: 0,
        }),
    }
}

async fn import_wallet(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ImportRequest>,
) -> Result<Json<MewWallet>, (axum::http::StatusCode, String)> {
    // 1. Пытаемся восстановить кошелек из ключа
    let wallet = MewWallet::import_from_secret(&payload.secret_key)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;

    // 2. Проверяем, есть ли он в базе, если нет — создаем начальные статы
    let _: () = state.db.fluent()
        .insert()
        .into("wallets")
        .document_id(&wallet.address)
        .object(&WalletStats {
            address: wallet.address.clone(),
            balance: 0.0,
            apy_earned: 0.0,
            tasks_completed: 0,
            last_claim: 0,
        })
        .execute::<()>()
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB Error".to_string()))?;

    Ok(Json(wallet))
}

async fn send_tokens(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SendRequest>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    // 1. Константы для экономики
    let admin_address = "mew013_ТВОЙ_АДРЕС_ТУТ"; // Замени на свой реальный адрес
    let fee_percent = 0.01; // 1% комиссия
    let fee = payload.amount * fee_percent;
    let total_deduction = payload.amount + fee;

    // 2. Получаем данные отправителя
    let mut sender_stats: WalletStats = state.db.fluent()
        .select()
        .by_id_in("wallets")
        .obj()
        .one(&payload.sender_address)
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB Error".to_string()))?
        .ok_or((axum::http::StatusCode::NOT_FOUND, "Sender not found".to_string()))?;

    // 3. Проверяем, хватает ли средств на перевод + комиссию
    if sender_stats.balance < total_deduction {
        return Err((axum::http::StatusCode::BAD_REQUEST, format!("Insufficient funds. Need {} MEW (incl. fee)", total_deduction)));
    }

    // 4. Получаем данные получателя
    let mut receiver_stats: WalletStats = state.db.fluent()
        .select()
        .by_id_in("wallets")
        .obj()
        .one(&payload.receiver_address)
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB Error".to_string()))?
        .ok_or((axum::http::StatusCode::NOT_FOUND, "Receiver not found".to_string()))?;

    // 5. Проводим расчеты балансов
    sender_stats.balance -= total_deduction;
    receiver_stats.balance += payload.amount;

    // 6. Обновляем отправителя в БД
    let _: () = state.db.fluent()
        .update()
        .fields(paths!(WalletStats::balance))
        .in_col("wallets")
        .document_id(&payload.sender_address)
        .object(&sender_stats)
        .execute::<()>()
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to update sender".to_string()))?;

    // 7. Обновляем получателя в БД
    let _: () = state.db.fluent()
        .update()
        .fields(paths!(WalletStats::balance))
        .in_col("wallets")
        .document_id(&payload.receiver_address)
        .object(&receiver_stats)
        .execute::<()>()
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to update receiver".to_string()))?;

    // 8. Зачисляем комиссию админу (тебе)
    let admin_opt: Option<WalletStats> = state.db.fluent()
        .select()
        .by_id_in("wallets")
        .obj()
        .one(admin_address)
        .await
        .unwrap_or(None);

    if let Some(mut admin_stats) = admin_opt {
        admin_stats.balance += fee;
        let db_c = state.db.clone();
        let addr_c = admin_address.to_string();
        tokio::spawn(async move {
            let _ = db_c.fluent()
                .update()
                .fields(paths!(WalletStats::balance))
                .in_col("wallets")
                .document_id(&addr_c)
                .object(&admin_stats)
                .execute::<()>()
                .await;
        });
    }

    // 9. Отчет в Telegram
    let msg = format!(
        "💸 *Перевод MEW*\nОт: `{}`\nКому: `{}`\nСумма: `{} MEW` (Газ: `{} MEW`)",
        payload.sender_address, payload.receiver_address, payload.amount, fee
    );
    let _ = state.bot.send_message(state.chat_id.clone(), msg).await;

    Ok(Json(format!("Successfully sent {} MEW (Fee: {})", payload.amount, fee)))
}

async fn complete_task(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CompleteTaskRequest>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    let collection = "wallets";

    // 1. Пытаемся получить текущую статистику кошелька
    let mut stats: WalletStats = state.db.fluent()
        .select()
        .by_id_in(collection)
        .obj()
        .one(&payload.address)
        .await
        .map_err(|e| {
            println!("Firestore Error: {:?}", e);
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Database connection error".into())
        })?
        .ok_or((axum::http::StatusCode::NOT_FOUND, "Wallet not found in system".into()))?;

    // 2. Логика проверки времени (Cooldown)
    let now = chrono::Utc::now().timestamp();
    let cooldown_seconds = 86400; // 24 часа для крана. Если хочешь 1 час — ставь 3600.
    
    let seconds_passed = now - stats.last_claim;

    if seconds_passed < cooldown_seconds {
        let remaining = cooldown_seconds - seconds_passed;
        let hours = remaining / 3600;
        let mins = (remaining % 3600) / 60;
        return Err((
            axum::http::StatusCode::FORBIDDEN, 
            format!("Cooldown active. Wait {}h {}m", hours, mins)
        ));
    }

    // 3. Определяем награду в зависимости от task_id
    let reward = if payload.task_id == "faucet_daily" {
        10.0 // Для крана игнорируем reward и ставим 10
    } else {
        payload.reward // А вот тут мы ЧИТАЕМ поле, и warning исчезнет!
    };

    // 4. Обновляем данные в структуре
    stats.balance += reward;
    stats.tasks_completed += 1;
    stats.last_claim = now;

    // 5. Сохраняем в Firestore (обновляем только нужные поля для безопасности)
    state.db.fluent()
        .update()
        .in_col(collection)
        .document_id(&payload.address)
        .object(&stats)
        .execute::<()>()
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to save data".into()))?;

    // 6. Отправляем красивое уведомление тебе в Telegram
    let tg_msg = format!(
        "💧 **Faucet Claimed!**\n\n👤 User: `{}`\n💰 Reward: `{} MEW`\n✅ Total Tasks: `{}`",
        payload.address, reward, stats.tasks_completed
    );
    let _ = state.bot.send_message(state.chat_id.clone(), tg_msg).await;

    Ok(Json(format!("Success! {} MEW added to your balance", reward)))
}

async fn create_group(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<JoinRequest>, // Используем JoinRequest для простоты
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    let now = chrono::Utc::now().timestamp();
    
    let new_group = NodeGroup {
        id: payload.group_id.clone(),
        owner: payload.address.clone(),
        members: vec![GroupMember {
            address: payload.address,
            joined_at: now,
            last_ping: now,
        }],
        total_mined: 0.0,
    };

    state.db.fluent()
        .insert().into("groups")
        .document_id(&new_group.id)
        .object(&new_group)
        .execute::<()>().await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB Error".to_string()))?;

    Ok(Json(format!("Группа {} создана", new_group.id)))
}

async fn node_ping(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<JoinRequest>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    let mut group: NodeGroup = state.db.fluent()
        .select().by_id_in("groups").obj().one(&payload.group_id).await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB Error".to_string()))?
        .ok_or((axum::http::StatusCode::NOT_FOUND, "Group not found".to_string()))?;

    let now = chrono::Utc::now().timestamp();
    
    // Ищем участника в векторе
    if let Some(member) = group.members.iter_mut().find(|m| m.address == payload.address) {
        // Проверка на 1 час (3600 сек)
        if now - member.last_ping < 3600 {
            return Err((axum::http::StatusCode::FORBIDDEN, "Too early for ping".to_string()));
        }

        // РАСЧЕТ НАГРАДЫ НА ОСНОВЕ ЛОЯЛЬНОСТИ
        // Базовая награда 0.1, +10% за каждый день пребывания (86400 сек), максимум +200%
        let days_in_group = (now - member.joined_at) / 86400;
        let loyalty_multiplier = 1.0 + (days_in_group as f64 * 0.1).min(2.0);
        let final_reward = 0.1 * loyalty_multiplier;

        member.last_ping = now;
        
        // Тут нужно вызвать функцию начисления баланса пользователю...
        // И обновить группу в БД
        state.db.fluent()
            .update().in_col("groups")
            .document_id(&group.id)
            .object(&group)
            .execute::<()>().await
            .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to save ping".to_string()))?;

        Ok(Json(format!("Пинг принят! Бонус лояльности: x{:.2}. Получено: {:.4}", loyalty_multiplier, final_reward)))
    } else {
        Err((axum::http::StatusCode::UNAUTHORIZED, "You are not in this group".to_string()))
    }
}

async fn join_group(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<JoinRequest>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    // 1. Ищем группу в БД
    let mut group: NodeGroup = state.db.fluent()
        .select()
        .by_id_in("groups")
        .obj()
        .one(&payload.group_id)
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB Error".to_string()))?
        .ok_or((axum::http::StatusCode::NOT_FOUND, "Group not found".to_string()))?;

    // 2. Проверяем, не в группе ли уже этот адрес
    if group.members.iter().any(|m| m.address == payload.address) {
        return Err((axum::http::StatusCode::BAD_REQUEST, "Already a member".to_string()));
    }

    // 3. Создаем нового участника с текущим временем (начало отсчета лояльности)
    let now = chrono::Utc::now().timestamp();
    let new_member = GroupMember {
        address: payload.address.clone(),
        joined_at: now,
        last_ping: now,
    };

    group.members.push(new_member);

    // 4. Сохраняем обновленный список участников в Firestore
    state.db.fluent()
        .update()
        .in_col("groups")
        .document_id(&group.id)
        .object(&group)
        .execute::<()>()
        .await
        .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to join group".to_string()))?;

    // 5. Уведомление в ТГ
    let msg = format!("👥 Новый участник в группе `{}`!\nАдрес: `{}`", group.id, payload.address);
    let _ = state.bot.send_message(state.chat_id.clone(), msg).await;

    Ok(Json(format!("Вы успешно вступили в группу {}", group.id)))
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
        .route("/wallet/new", get(create_wallet))
        .route("/wallet/import", post(import_wallet))
        .route("/wallet/:address", get(get_wallet_stats))
        .route("/wallet/task", post(complete_task))
        .route("/wallet/send", post(send_tokens))
        .route("/groups/create", post(create_group))
        .route("/groups/join", post(join_group))
        .route("/groups/ping", post(node_ping))
        .fallback_service(ServeDir::new("static")) // Раздаем статику
        .with_state(shared_state); // Состояние прикрепляем ОДИН РАЗ в самом конце
        
    // Запуск сервера
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("🚀 MewChain Core запущена на {}", addr);

    axum::serve(listener, app).await.unwrap();
}
