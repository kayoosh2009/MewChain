use serde::{Serialize, Deserialize};
use sha2::{Sha256, Digest};
use chrono::prelude::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Block {
    index: u32,
    timestamp: i64,
    data: String,
    previous_hash: String,
    hash: String,
    nonce: u64, // Число-счетчик для майнинга
}

impl Block {
    fn new(index: u32, data: String, previous_hash: String) -> Self {
        let timestamp = Utc::now().timestamp();
        let mut block = Block {
            index,
            timestamp,
            data,
            previous_hash,
            hash: String::new(),
            nonce: 0,
        };
        block.mine(2); // Сложность майнинга: хеш должен начинаться с "00"
        block
    }

    fn calculate_hash(&self) -> String {
        let mut hasher = Sha256::new();
        let input = format!(
            "{}{}{}{}{}",
            self.index, self.timestamp, self.data, self.previous_hash, self.nonce
        );
        hasher.update(input);
        format!("{:x}", hasher.finalize())
    }

    // Механизм майнинга: перебираем nonce, пока не найдем нужный хеш
    fn mine(&mut self, difficulty: usize) {
        let target = "0".repeat(difficulty);
        while !self.hash.starts_with(&target) {
            self.nonce += 1;
            self.hash = self.calculate_hash();
        }
        println!("Блок замайнен! Хеш: {}", self.hash);
    }
}

struct Blockchain {
    chain: Vec<Block>,
}

impl Blockchain {
    fn new() -> Self {
        let genesis_block = Block::new(0, "Genesis Block".to_string(), "0".to_string());
        Blockchain { chain: vec![genesis_block] }
    }

    fn add_block(&mut self, data: String) {
        let previous_hash = self.chain.last().unwrap().hash.clone();
        let new_block = Block::new(self.chain.len() as u32, data, previous_hash);
        self.chain.push(new_block);
    }
}

fn main() {
    let mut my_blockchain = Blockchain::new();
    
    println!("Добавляем блок 1...");
    my_blockchain.add_block("Перевод: Alice -> Bob (10 coins)".to_string());

    println!("Добавляем блок 2...");
    my_blockchain.add_block("Перевод: Bob -> Charlie (5 coins)".to_string());

    println!("\nИтоговая цепочка:");
    for block in my_blockchain.chain {
        println!("Индекс: {}, Хеш: {}, Nonce: {}", block.index, block.hash, block.nonce);
    }
}