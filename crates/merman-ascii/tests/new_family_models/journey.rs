use super::*;

#[test]
fn journey_render_model_renders_actors_sections_and_scores() {
    let mut model = JourneyDiagramRenderModel::default();
    model.title = Some("Journey".to_string());
    model.acc_title = Some("Journey title".to_string());
    model.acc_descr = Some("Journey description".to_string());
    model.sections = vec!["Discovery".to_string()];
    model.tasks = vec![
        JourneyRenderTask {
            score: 5.0,
            score_is_nan: false,
            people: vec!["Alice".to_string(), "Bob".to_string()],
            section: "Discovery".to_string(),
            section_index: Some(0),
            task_type: "Discovery".to_string(),
            task: "Research".to_string(),
        },
        JourneyRenderTask {
            score: 3.0,
            score_is_nan: false,
            people: vec!["Bob".to_string()],
            section: "Discovery".to_string(),
            section_index: Some(0),
            task_type: "Discovery".to_string(),
            task: "Ship".to_string(),
        },
    ];

    let rendered = render(RenderSemanticModel::Journey(model));

    assert_eq!(
        rendered,
        concat!(
            "title(bytes=7)=\"Journey\"\n",
            "accTitle(bytes=13)=\"Journey title\"\n",
            "accDescr(bytes=19)=\"Journey description\"\n",
            "actors=[bytes=5 \"Alice\", bytes=3 \"Bob\"]\n",
            "section(bytes=9)=\"Discovery\"\n",
            "  - task(bytes=8)=\"Research\" score=5 people=[bytes=5 \"Alice\", bytes=3 \"Bob\"]\n",
            "  - task(bytes=4)=\"Ship\" score=3 people=[bytes=3 \"Bob\"]",
        )
    );
}
#[test]
fn journey_structured_text_framing_distinguishes_actor_list_items() {
    let mut one_actor = JourneyDiagramRenderModel::default();
    one_actor.actors = vec!["Alice, Bob".to_string()];

    let mut two_actors = JourneyDiagramRenderModel::default();
    two_actors.actors = vec!["Alice".to_string(), "Bob".to_string()];

    assert_ne!(
        render(RenderSemanticModel::Journey(one_actor)),
        render(RenderSemanticModel::Journey(two_actors)),
        "one authored actor containing a comma must differ from two actor values",
    );
}

#[test]
fn journey_numeric_scores_keep_fraction_radix_infinity_and_nan_meaning_in_ascii() {
    let output = render_parsed(
        "journey\nFraction: 3.5\nHex: 0x5\nPositive: Infinity\nNegative: -Infinity\nInvalid: inf\n",
    );
    for expected in [
        "Fraction\" score=3.5",
        "Hex\" score=5",
        "Positive\" score=Infinity",
        "Negative\" score=-Infinity",
        "Invalid\" score=NaN",
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
}
