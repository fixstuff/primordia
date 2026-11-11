# LifeAPI league

6 of 6 matches played. Elo is applied in schedule order (it depends on order); a player is rated only in matches it played; K = 32/(players in the match - 1). Mean rank and share mix field sizes when the field changes. Costs: Anthropic as reported, OpenAI estimated from tokens, DeepSeek not priced.

| Rank | Player | Elo | Wins | Mean rank | Mean share of top score | API cost (est. for OpenAI) |
|---|---|---|---|---|---|---|
| 1 | claude-fable-5-2 | 1541 | 3/6 | 2.17 | 63% | $38.54 |
| 2 | claude-opus-5-5 | 1526 | 1/6 | 2.33 | 48% | $3.56 |
| 3 | gpt-5.5 | 1510 | 1/3 | 3.00 | 41% | $5.42 |
| 4 | gpt-6-astra | 1509 | 0/3 | 3.00 | 40% | $3.91 |
| 5 | deepseek-v4-pro | 1489 | 1/6 | 3.17 | 31% | — |
| 6 | scripted-bot | 1424 | 0/6 | 4.83 | 1% | — |

| Match | World | Seed | 1st | 2nd | 3rd | 4th | 5th | 6th |
|---|---|---|---|---|---|---|---|---|
| m01-drift-13 | drift | 13 | claude-fable-5-2 (5005) | claude-opus-5-5 (1853) | scripted-bot (225) | deepseek-v4-pro (195) |
| m02-patchy-13 | patchy | 13 | claude-fable-5-2 (6171) | claude-opus-5-5 (1474) | deepseek-v4-pro (206) | scripted-bot (25) |
| m03-bloom-13 | bloom | 13 | claude-fable-5-2 (3852) | claude-opus-5-5 (2015) | deepseek-v4-pro (1465) | scripted-bot (76) |
| m04-drift-14 | drift | 14 | claude-opus-5-5 (2968) | gpt-6-astra (1293) | claude-fable-5-2 (512) | gpt-5.5 (264) | deepseek-v4-pro (57) | scripted-bot (0) |
| m05-patchy-14 | patchy | 14 | deepseek-v4-pro (3222) | claude-opus-5-5 (1642) | claude-fable-5-2 (866) | gpt-5.5 (491) | gpt-6-astra (386) | scripted-bot (5) |
| m06-bloom-14 | bloom | 14 | gpt-5.5 (2782) | gpt-6-astra (1780) | deepseek-v4-pro (1012) | claude-fable-5-2 (986) | claude-opus-5-5 (606) | scripted-bot (6) |
