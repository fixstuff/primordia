# LifeAPI league

6 of 6 matches played. Elo is applied in schedule order (it depends on order); a player is rated only in matches it played; K = 32/(players in the match - 1). Mean rank and share mix field sizes when the field changes. Costs: Anthropic as reported, OpenAI estimated from tokens, DeepSeek not priced.

| Rank | Player | Elo | Wins | Mean rank | Mean share of top score | API cost (est. for OpenAI) |
|---|---|---|---|---|---|---|
| 1 | claude-fable-5-2 | 1537 | 2/6 | 2.33 | 72% | $43.65 |
| 2 | gpt-6-astra | 1534 | 2/6 | 2.50 | 59% | $7.90 |
| 3 | claude-opus-5-5 | 1528 | 0/6 | 2.67 | 51% | $4.15 |
| 4 | gpt-5.5 | 1500 | 1/6 | 3.50 | 29% | $12.79 |
| 5 | deepseek-v4-pro | 1486 | 1/6 | 4.00 | 25% | — |
| 6 | scripted-bot | 1416 | 0/6 | 6.00 | 0% | — |

| Match | World | Seed | 1st | 2nd | 3rd | 4th | 5th | 6th |
|---|---|---|---|---|---|---|---|---|
| m01-drift-15 | drift | 15 | gpt-6-astra (1991) | claude-fable-5-2 (1439) | claude-opus-5-5 (170) | gpt-5.5 (36) | deepseek-v4-pro (23) | scripted-bot (3) |
| m02-patchy-15 | patchy | 15 | claude-fable-5-2 (3429) | claude-opus-5-5 (2576) | deepseek-v4-pro (524) | gpt-6-astra (514) | gpt-5.5 (180) | scripted-bot (6) |
| m03-bloom-15 | bloom | 15 | gpt-5.5 (2075) | claude-opus-5-5 (1456) | claude-fable-5-2 (1151) | gpt-6-astra (1148) | deepseek-v4-pro (581) | scripted-bot (0) |
| m04-drift-16 | drift | 16 | gpt-6-astra (1908) | claude-fable-5-2 (1472) | gpt-5.5 (433) | claude-opus-5-5 (279) | deepseek-v4-pro (37) | scripted-bot (3) |
| m05-patchy-16 | patchy | 16 | claude-fable-5-2 (4146) | claude-opus-5-5 (3970) | gpt-6-astra (399) | gpt-5.5 (172) | deepseek-v4-pro (120) | scripted-bot (44) |
| m06-bloom-16 | bloom | 16 | deepseek-v4-pro (2465) | gpt-6-astra (1890) | claude-opus-5-5 (1061) | gpt-5.5 (929) | claude-fable-5-2 (662) | scripted-bot (2) |
