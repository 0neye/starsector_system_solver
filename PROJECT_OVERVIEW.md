# Starsector System Ranker 2 - Project Overview

## Project Purpose

This is a Rust-based optimization solver for **Starsector**, a space exploration and colony management game. The project finds optimal colony development strategies by searching through possible action sequences to maximize income, stability, and defense metrics while respecting resource constraints.

## Core Architecture

### Main Components

1. **State Management** (`src/solver/state.rs`)
   - `State`: Represents the complete game state (balance, system, action log)
   - `Balance`: Tracks credits, story points, alpha cores, and colony items
   - `Action`: Enum of all possible game actions (colonize, build facilities, wait, etc.)
   - Supports reversible actions for efficient search tree traversal

2. **Search Algorithms** (`src/solver/mod.rs`, `src/solver/astar.rs`)
   - **DFS (Depth-First Search)**: Iterative deepening search with transposition tables
   - **IDA* (Iterative Deepening A*)**: Goal-oriented search with admissible heuristics
   - Both algorithms use action sequence hashing to detect duplicate states
   - Time-limited search with configurable depth limits

3. **Game Logic**
   - **System** (`src/system/mod.rs`): Represents a star system with planets and infrastructure
   - **Planet** (`src/planet/mod.rs`): Models individual planets with:
     - Colony size and growth mechanics
     - Facilities and their states
     - Resource production and consumption
     - Income, stability, and defense calculations
   - **Facility** (`src/planet/facility.rs`): Individual facilities with:
     - Build progress tracking
     - Improvements and alpha cores
     - Colony item installations
     - Production/demand calculations

4. **Data Loading** (`src/parser/mod.rs`)
   - CSV parsing for planets and infrastructure
   - Loads game data from `Planets.csv` and `Infrastructure.csv`

5. **Constants** (`src/constants.rs`)
   - Facility definitions (build costs, build times, production/demands)
   - Colony item effects
   - Resource definitions and market values
   - Admin bonuses

## Key Features

### Search Optimization

- **Transposition Tables**: Uses hash sets to avoid exploring duplicate states
- **Action Sequence Hashing**: Efficiently hashes action sequences rather than full state
- **Slim Mode**: Reduces search space by limiting certain action types
- **Parallel Processing**: Uses Rayon for parallel planet searches (`search_all_planets`)

### Game Mechanics Modeling

- **Colony Growth**: Tracks colony size (3-6) and growth progress over time
- **Facility Building**: Models construction times and costs
- **Resource Economy**: Tracks production, consumption, and market values
- **Income Calculation**: Computes gross/net income based on accessibility, stability, and facilities
- **Defense & Stability**: Calculates ground defense strength and stability bonuses

### Action Types

The solver supports these actions:
- `Colonize`: Establish a new colony (costs 125,000 credits)
- `AddFacility`: Build a new facility
- `AddImprovement`: Add facility improvements (costs story points)
- `AddAlphaCore`: Install alpha core (costs alpha cores)
- `InstallItem`: Install colony items (e.g., nanoforges, biofactories)
- `SetFreePort`: Toggle free port status
- `SetHazardPay`: Toggle hazard pay
- `UpgradeAdmin`: Upgrade from Base to Alpha Core admin
- `Wait`: Advance time (months) to allow growth and construction

## Search Strategy

### Current Implementation

1. **Iterative Deepening**: Searches at increasing depths (2 to MAX_DEPTH=100)
2. **Alpha-Beta Pruning**: Uses alpha cutoff to prune branches
3. **Goal-Oriented Search**: IDA* implementation with heuristic estimates for:
   - Minimum months to reach target net income
   - Minimum months to reach target defense
   - Minimum months to reach target stability

### Planned Improvements (from TODOs)

- Multi-phase search: first without facility improvements, then full search
- Per-planet optimization: search planets individually, then combine results
- Heuristic-guided search using precomputed facility value data

## Data Structures

### Performance Optimizations

- **Custom Hashers**: Uses `nohash-hasher` and `rustc-hash` for fast hashing
- **Efficient Collections**: Uses `FxHashMap` and `FxHashSet` where appropriate
- **Action Prioritization**: Actions are sorted by priority to explore promising branches first

### State Representation

- Planets identified by name hash (u64) for fast lookup
- Facilities stored in vectors with efficient lookups
- Action logs maintain full history for undo operations

## File Structure

```
src/
├── main.rs              # Entry point, test harness
├── lib.rs               # Library exports
├── constants.rs         # Game constants and data definitions
├── utils.rs             # Utility functions (formula parser)
├── parser/
│   └── mod.rs          # CSV parsing for game data
├── system/
│   └── mod.rs          # System representation
├── planet/
│   ├── mod.rs          # Planet implementation
│   └── facility.rs     # Facility implementation
├── solver/
│   ├── mod.rs          # Main search algorithm (DFS)
│   ├── state.rs        # State and action definitions
│   └── astar.rs        # IDA* search implementation
└── tests/              # In-crate test suite (cfg(test) only)
    ├── mod.rs          # Suite root + conventions
    ├── support.rs      # Shared fixtures and builders
    ├── planet.rs       # Colony facility gating / production
    ├── facility.rs     # Facility construction bookkeeping
    ├── solver.rs       # Action hashing + apply/undo invariants
    └── system.rs       # System-wide aggregates
```

## Testing

Run the suite with `cargo test`.

Tests live **inside the crate** under `src/tests/` (compiled only under
`cfg(test)`) rather than in a top-level `tests/` directory, because they assert
on `pub(crate)` internals — facility build-day counters, the solver's
apply/undo round-trip — that an external integration test could not reach.

Layout mirrors `src/`: each test file targets the module it shares a name with,
and shared setup lives in `support.rs` (`PlanetBuilder`, `colonized_state`,
`apply_all`, …). To add a test: reuse or extend a builder in `support.rs`, drop
the test in the file matching the module under test, and name it after the
behavior it guards. Tests that lock in a specific bug fix note the regression
origin in a doc comment. See `src/tests/mod.rs` for the full convention.

## Usage Example

```rust
// Load game data
let systems = parser::load_game_data("Planets.csv", "Infrastructure.csv")?;
let test_system = systems.get("Mia Bravos").unwrap().clone();

// Create initial balance
let initial_balance = Balance::new(2_000_000.0, 5, 5);

// Create initial state
let mut state = State::new(initial_balance, test_system);

// Define goal
let goal = Goal::new(20_000.0, None, Some(8)); // 20k net income, stability >= 8

// Run search
let result = search_all_planets(&mut state, &goal, 50_000, true);
```

## Dependencies

- `lazy_static`: For static data initialization
- `csv`: CSV file parsing
- `rayon`: Parallel processing
- `rustc-hash`: Fast hashing
- `nohash-hasher`: Integer-based hashing

## Search Space Complexity

The project includes `tree_estimator.py` which estimates the search space size:
- Planets start with 2 facilities, can have up to 11
- Each facility can have 2 types of improvements (max 4 per type per planet)
- With 2 planets, the estimated configuration space is extremely large (quintillions of nodes)

This is why the solver uses:
- Transposition tables to avoid duplicates
- Time limits and depth limits
- Heuristic pruning
- Parallel per-planet searches

## Current Status

The project is functional with:
- ✅ Complete game mechanics modeling
- ✅ DFS and IDA* search implementations
- ✅ Reversible action system
- ✅ CSV data loading
- ✅ Goal-oriented search with heuristics

Planned improvements (from code TODOs):
- Rework facility upgrading/downgrading for less reallocation
- Fix colony growth reversibility issues
- Implement multi-phase search strategies
- Add system-wide restrictions (e.g., commerce facility limits)



