# Harmion

一人のユーザーが所有する、アプリから独立した個人データ基盤。

現時点では設計段階です。次の文書は実装済み機能ではなく、会話で合意した仕様と設計方針を記録しています。

- [現時点の仕様と設計方針](docs/specification.md)
- [過去の議論との照合・整合性レビュー](docs/review.md)
- [内部表現と同期の初期設計](docs/internal-design.md)
- [ArchitectureとRustパッケージ構成](docs/architecture.md)
- [取得・編集・Subscribe・OAuthの契約](docs/app-api.md)
- [設計の実行可能な意味モデル](design_checks/model_check.py)

内部表現と同期の初期方式を選定し、小規模な意味モデルで到着順・再送・障害時の挙動を検証しています。実システムは未実装です。未決定事項を確定仕様として扱わず、仕様・内部設計・レビューを併せて参照してください。

モデル検証はPython 3.10以降で `python3 design_checks/model_check.py` を実行できます。外部パッケージは不要です。
