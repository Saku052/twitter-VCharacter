import os
import re
from pathlib import Path

from fastapi import FastAPI, Header, HTTPException
from claude_agent_sdk import ClaudeAgentOptions, query, AssistantMessage, TextBlock, ResultMessage

APP_DIR = Path(__file__).resolve().parent

# Agentの最終出力は「メモ2個を改行区切り」とTASK_PROMPTで指示しているが、
# 実際には前置き・見出し・参考リンクが混ざることがある（実測: 35日中9日）。
# プロンプトの遵守を前提にせず、採用前にコード側で弾く。
EXPECTED_MEMO_COUNT = 2
MEMO_MAX_LEN = 80  # CLAUDE.mdの指示は50字以内。表記ゆれを見込んで余裕を持たせる

# 1つでも該当したらメモではないと判断するパターン
_REJECT_PATTERNS = [
    re.compile(r"https?://"),          # 参考リンク行。URL付き投稿はX APIの単価が13倍になる
    re.compile(r"^[-*・#>]"),          # 箇条書き・見出し記号で始まる行
    re.compile(r"^\d+[.)]"),           # 「1. 」のような番号付け
    re.compile(r"\*\*"),               # **メモ1（…）** のような強調見出し
    re.compile(r"以下[、,：:]|以下の\d+|以下です"),  # 「以下、最終メモ2個です」「〜しました。以下、2つのメモです」
    re.compile(r"^メモ\s*\d"),         # 「メモ1」「メモ 2」
    re.compile(r"CLAUDE\.md|TASK_PROMPT|WebSearch"),  # 内部の仕組みへの言及
    re.compile(r"文字以内|字以内|収まって|作成しました|作成します|深掘りしました"),  # 自己申告・前置き
]


def _is_valid_memo(line: str) -> bool:
    if not (0 < len(line) <= MEMO_MAX_LEN):
        return False
    return not any(p.search(line) for p in _REJECT_PATTERNS)

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

    lines = [line.strip() for line in last_text.splitlines() if line.strip()]
    memos = [line for line in lines if _is_valid_memo(line)]

    rejected = [line for line in lines if line not in memos]
    if rejected:
        print(f"メモとして不採用: {rejected}", flush=True)

    if not memos:
        raise RuntimeError(f"有効なメモが得られませんでした: {lines}")

    # 想定より多い場合は前置き等が残っている可能性があるため、先頭から必要数だけ採る。
    # 少ない場合は「0件よりは投入する」方針でそのまま通す（供給が細いため）
    if len(memos) > EXPECTED_MEMO_COUNT:
        print(f"メモが{len(memos)}件あったため先頭{EXPECTED_MEMO_COUNT}件を採用", flush=True)
        memos = memos[:EXPECTED_MEMO_COUNT]

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
