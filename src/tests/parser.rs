//! CSV-loading regression tests against the bundled game data.

use crate::parser::load_game_data;
use crate::system::Infrastructure;

/// The bundled `Infrastructure.csv` spells nav buoys with Starsector's internal
/// typo "NavBouy" (e.g. the Mia Bravos row). The parser must accept that spelling
/// — matching only "NavBuoy" silently dropped the row into the unknown-type arm,
/// so the system lost its nav buoy. (Regression: P2 nav-buoy parsing.)
#[test]
fn nav_buoy_csv_spelling_loads() {
    let systems = load_game_data("Planets.csv", "Infrastructure.csv")
        .expect("game data CSVs load from the crate root during tests");

    let mia = systems
        .get("Mia Bravos")
        .expect("Mia Bravos is present in Planets.csv");

    let has_nav_buoy = mia
        .infrastructure()
        .values()
        .any(|i| matches!(i, Infrastructure::NavBuoy { .. }));

    assert!(
        has_nav_buoy,
        "Mia Bravos should expose its NavBouy infrastructure after loading"
    );
}
