# 🎯 Multiplayer Number Guessing Game (Backend - Rust)

A **real-time multiplayer number guessing game** backend built in **Rust**, using [Axum](https://docs.rs/axum/latest/axum/) and [Tokio](https://tokio.rs/) for high-performance async handling.  
Players connect via WebSockets to compete in guessing a secret number in the fewest attempts possible.  

> This project was created as a learning initiative to gain hands-on experience with Rust backend development, WebSocket communication, and modern async programming.

---

## 🚀 Features

- **Real-Time Multiplayer** — WebSocket server to handle multiple players simultaneously.
- **Core Game Logic** — Competitive number guessing gameplay with session tracking.
- **Efficient State Management** — Track player progress and results in memory for fast performance.
- **High Concurrency** — Powered by Axum and Tokio to support multiple concurrent connections with minimal latency.
- **Clean, Modular Code** — Follows Rust best practices for maintainability and scalability.

---

## 🛠 Tech Stack

- **Language**: Rust 🦀
- **Framework**: [Axum](https://docs.rs/axum/latest/axum/)
- **Async Runtime**: [Tokio](https://tokio.rs/)
- **Communication**: WebSockets
- **Build Tool**: Cargo
