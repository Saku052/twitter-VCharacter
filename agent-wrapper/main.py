import os
from pathlib import Path

from fastapi import FastAPI, Header, HTTPException
from claude_agent_sdk import ClaudeAgentOptions, query, AssistantMessage, TextBlock, ResultMessage

APP_DIR = Path(__file__).resolve().parent

TASK_PROMPT = """今日の技術トレンドを1つ調べて、メモを1個作成してください。

## 手順

### Step 1: 広く探る（検索2〜3回）
まず短く広いクエリで、今どんな技術トピックが話題になっているか全体像をつかんでください。
例: 「プログラミング トレンド 2026」「AI開発 話題」など。
特定の技術・製品名を最初から狙い撃ちしないこと。

### Step 2: 絞り込む（検索最大3回）
Step 1で見つけた候補の中から、気になったものを1つ選び、内容を深掘りしてください。
複数の候補を並行して深掘りしない。1つに決めてから掘る。

### Step 3: メモを確定する
選んだトピックについて、CLAUDE.mdのルールに従ってメモを1個作成し、それを最終出力としてください。
メモが完成したら、それ以上の検索は行わないこと。

## 終了条件
- メモ1個を出力したら終了
- 合計10ターンを超えたら、その時点までの情報で必ずメモを1個確定させて終了する（探索を続けない）
- 「これ以上良いネタが見つかるかもしれない」という理由だけで探索を継続しないこと

## 出力フォーマット（重要）
最後の発言は、必ずメモの本文だけにしてください。
「メモを作成しました」のような前置き、見出し（**メモ**等）、選定理由の説明を一切含めないこと。
最後の発言＝そのままメモ本文として使われます。
"""

app = FastAPI()


async def run_investigation() -> str:
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

    memo: str | None = None
    result_message: ResultMessage | None = None

    async for message in query(prompt="開始してください。", options=options):
        if isinstance(message, AssistantMessage):
            text_blocks = [b.text for b in message.content if isinstance(b, TextBlock)]
            if text_blocks:
                memo = text_blocks[-1].strip()
        elif isinstance(message, ResultMessage):
            result_message = message

    if result_message is None or result_message.is_error:
        raise RuntimeError(f"Agent実行がエラー終了しました: {result_message}")

    if not memo:
        raise RuntimeError("Agentからメモが得られませんでした")

    return memo


@app.post("/investigate")
async def investigate(x_api_key: str | None = Header(default=None)):
    expected_key = os.environ.get("AGENT_SDK_API_KEY")
    if not expected_key or x_api_key != expected_key:
        raise HTTPException(status_code=401, detail="unauthorized")

    try:
        memo = await run_investigation()
    except Exception as e:
        raise HTTPException(status_code=502, detail=f"investigation failed: {e}")

    return {"memo": memo}
