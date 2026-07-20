import os
from pathlib import Path

from fastapi import FastAPI, Header, HTTPException
from claude_agent_sdk import ClaudeAgentOptions, query, AssistantMessage, TextBlock, ResultMessage

APP_DIR = Path(__file__).resolve().parent

TASK_PROMPT = """今日のゲーム開発関連の話題を2つ調べて、メモを2個作成してください。

## 手順

### Step 1: 広く探る（検索2〜3回）
まず短く広いクエリで、今どんなゲーム開発関連のトピックが話題になっているか全体像をつかんでください。
例: 「インディーゲーム開発 話題」「ゲームデザイン トレンド 2026」「個人ゲーム開発 Tips」など。
特定の技術・製品名を最初から狙い撃ちしないこと。

### Step 2: 絞り込む（検索最大3回）
Step 1で見つけた候補の中から、気になったものを2つ選び、それぞれ内容を深掘りしてください。
2つは別々のトピックにすること（同じ話題の言い換えにしない）。

### Step 3: メモを確定する
選んだ2つのトピックについて、CLAUDE.mdのルールに従ってメモをそれぞれ1個ずつ、合計2個作成し、それを最終出力としてください。
メモが完成したら、それ以上の検索は行わないこと。

## 終了条件
- メモ2個を出力したら終了
- 合計10ターンを超えたら、その時点までの情報で必ずメモを2個確定させて終了する（探索を続けない）
- 「これ以上良いネタが見つかるかもしれない」という理由だけで探索を継続しないこと

## 出力フォーマット（重要）
最後の発言は、必ずメモ2個を1行ずつ、改行区切りで出力してください。
前置き、見出し（**メモ**等）、選定理由の説明、番号付け（「1. 」等）を一切含めないこと。
最後の発言の各行＝そのままメモ本文として使われます。
"""

app = FastAPI()


async def run_investigation() -> list[str]:
    options = ClaudeAgentOptions(
        cwd=str(APP_DIR),
        setting_sources=["project"],
        allowed_tools=["WebSearch"],
        max_turns=10,
        model="claude-sonnet-5",
        effort="high",
        system_prompt={
            "type": "preset",
            "preset": "claude_code",
            "append": TASK_PROMPT,
        },
    )

    last_text: str | None = None
    result_message: ResultMessage | None = None

    async for message in query(prompt="開始してください。", options=options):
        if isinstance(message, AssistantMessage):
            text_blocks = [b.text for b in message.content if isinstance(b, TextBlock)]
            if text_blocks:
                last_text = text_blocks[-1].strip()
        elif isinstance(message, ResultMessage):
            result_message = message

    if result_message is None or result_message.is_error:
        raise RuntimeError(f"Agent実行がエラー終了しました: {result_message}")

    if not last_text:
        raise RuntimeError("Agentからメモが得られませんでした")

    memos = [line.strip() for line in last_text.splitlines() if line.strip()]
    if not memos:
        raise RuntimeError("Agentからメモが得られませんでした")

    return memos


@app.post("/investigate")
async def investigate(x_api_key: str | None = Header(default=None)):
    expected_key = os.environ.get("AGENT_SDK_API_KEY")
    if not expected_key or x_api_key != expected_key:
        raise HTTPException(status_code=401, detail="unauthorized")

    try:
        memos = await run_investigation()
    except Exception as e:
        raise HTTPException(status_code=502, detail=f"investigation failed: {e}")

    return {"memos": memos}
