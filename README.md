# CLMM backtester (WIP - not ready)

Will probably be done in 10 days.

For now I am building a backtesting engine for CLMM AMMs. The first ones to be done are Orca and then maybe Raydium. Later one I might touch some EVM ones like PancakeSwap (but starting with hardest first)

Currently I'm focusing on the transaction syncer to fetch all relevant solana txs for Orca pools. After successfully integrating the main syncer logic I will focus on:
- Liquidity range builder from positions
- Transaction engine replayer to calculate revenue from your position
- Strategy injection (no strat, time rebalance, volatility rebalance, hedging accountability, etc)


