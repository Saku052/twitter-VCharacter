//! バイナリ間（本体 / planner / tick）で共有する実装。
//! main.rs と各 bin から同じモジュールを参照するためにライブラリとして公開する。

pub mod adapters;
pub mod config;
pub mod domain;
pub mod ports;
pub mod publish;
