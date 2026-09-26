# repo: https://github.com/Aider-AI/aider (Apache-2.0)
# commit: 5dc9490bb35f9729ef2c95d00a19ccd30c26339c  committed: 2026-05-22T14:02:20Z  retrieved: 2026-09-22
# path: aider/args.py  lines 270-300
    group = parser.add_argument_group("History Files")
    default_input_history_file = (
        os.path.join(git_root, ".aider.input.history") if git_root else ".aider.input.history"
    )
    default_chat_history_file = (
        os.path.join(git_root, ".aider.chat.history.md") if git_root else ".aider.chat.history.md"
    )
    group.add_argument(
        "--input-history-file",
        metavar="INPUT_HISTORY_FILE",
        default=default_input_history_file,
        help=f"Specify the chat input history file (default: {default_input_history_file})",
    ).complete = shtab.FILE
    group.add_argument(
        "--chat-history-file",
        metavar="CHAT_HISTORY_FILE",
        default=default_chat_history_file,
        help=f"Specify the chat history file (default: {default_chat_history_file})",
    ).complete = shtab.FILE
    group.add_argument(
        "--restore-chat-history",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Restore the previous chat history messages (default: False)",
    )
    group.add_argument(
        "--llm-history-file",
        metavar="LLM_HISTORY_FILE",
        default=None,
        help="Log the conversation with the LLM to this file (for example, .aider.llm.history)",
    ).complete = shtab.FILE
