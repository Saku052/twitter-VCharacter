use vcharacter::publish::publish_once;

/// 失敗を Railway 上で FAILED として見えるようにする
/// （終了コード0のままだと SUCCESS 表示になり気づけない）
#[tokio::main]
async fn main() {
    match publish_once().await {
        Ok(_) => println!("完了！"),
        Err(e) => {
            eprintln!("エラー: {:#}", e);
            std::process::exit(1);
        }
    }
}
