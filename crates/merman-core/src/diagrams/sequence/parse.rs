use crate::{
    EditorExpectedSyntax, EditorExpectedSyntaxKind, EditorSemanticFacts, EditorSemanticKind,
    EditorSemanticSymbol, Error, OperationControl, OperationControlResult, ParseMetadata, Result,
    SourceSpan,
    editor::{format_lalrpop_parse_error, lalrpop_parse_diagnostic, lalrpop_recovery_span},
};
use serde_json::Value;
#[cfg(test)]
use std::cell::Cell;
use std::collections::{HashMap, hash_map::Entry};

use super::SequenceDiagramRenderModel;
use super::Tok;
use super::db::{SequenceDb, is_css_color_value, split_box_color_and_title};
use super::lexer::Lexer;
use super::sequence_grammar;

#[cfg(test)]
thread_local! {
    static SEQUENCE_SYNTAX_CONSTRUCTION_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_sequence_syntax_construction_count() {
    SEQUENCE_SYNTAX_CONSTRUCTION_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn sequence_syntax_construction_count() -> usize {
    SEQUENCE_SYNTAX_CONSTRUCTION_COUNT.get()
}

type SequenceLexicalEvent = std::result::Result<(usize, Tok, usize), super::LexError>;
type SequenceGrammarError = lalrpop_util::ParseError<usize, Tok, super::LexError>;

struct SequenceSyntax {
    events: Vec<SequenceLexicalEvent>,
}

impl SequenceSyntax {
    fn lex(code: &str, control: &OperationControl) -> OperationControlResult<Self> {
        #[cfg(test)]
        SEQUENCE_SYNTAX_CONSTRUCTION_COUNT.set(SEQUENCE_SYNTAX_CONSTRUCTION_COUNT.get() + 1);

        let mut events = Vec::new();
        let mut lexer = Lexer::new(code);
        let mut last_position = lexer.position();
        while let Some(event) = lexer.next() {
            if events.len() % 128 == 0 {
                control.checkpoint()?;
            }
            let current_position = lexer.position();
            let must_stop = event.is_err() && current_position == last_position;
            events.push(event);
            if must_stop {
                break;
            }
            last_position = current_position;
        }
        control.checkpoint()?;
        Ok(Self { events })
    }

    fn into_editor_facts_and_actions(
        self,
        code: &str,
        control: &OperationControl,
    ) -> OperationControlResult<(
        EditorSemanticFacts,
        std::result::Result<Vec<super::Action>, SequenceGrammarError>,
        Vec<SequenceLexicalEvent>,
    )> {
        let Self { events } = self;
        let editor_facts = collect_sequence_editor_facts_from_events(&events, code, control)?;
        control.checkpoint()?;
        let mut emitted = 0usize;
        let controlled_events = events.iter().cloned().take_while(|_| {
            let active = !emitted.is_multiple_of(128) || !control.is_cancelled();
            emitted = emitted.saturating_add(1);
            active
        });
        let actions = sequence_grammar::ActionsParser::new().parse(controlled_events);
        control.checkpoint()?;
        Ok((editor_facts, actions, events))
    }
}

struct SequenceSemanticSource {
    db: SequenceDb,
    editor_facts: EditorSemanticFacts,
}

enum SequenceSemanticFailure {
    Grammar {
        error: SequenceGrammarError,
        editor_facts: EditorSemanticFacts,
    },
    Db {
        message: String,
        editor_facts: EditorSemanticFacts,
    },
}

impl SequenceSemanticFailure {
    fn into_parse_error(self, meta: &ParseMetadata, fallback_offset: usize) -> Error {
        self.into_error_and_editor_facts(meta, fallback_offset).0
    }

    fn into_error_and_editor_facts(
        self,
        meta: &ParseMetadata,
        fallback_offset: usize,
    ) -> (Error, EditorSemanticFacts) {
        self.into_error_and_editor_facts_for_type(&meta.diagram_type, fallback_offset)
    }

    fn into_error_and_editor_facts_for_type(
        self,
        diagram_type: &str,
        fallback_offset: usize,
    ) -> (Error, EditorSemanticFacts) {
        match self {
            Self::Grammar {
                error,
                mut editor_facts,
            } => {
                let parse_error = Error::diagram_parse_diagnostic(
                    diagram_type.to_string(),
                    lalrpop_parse_diagnostic(&error, fallback_offset),
                );
                let span = match &error {
                    lalrpop_util::ParseError::User { error } => error.span,
                    _ => lalrpop_recovery_span(&error, fallback_offset),
                };
                editor_facts.mark_recovered_from_parse_error(
                    format!(
                        "sequence parser recovered after parse error: {}",
                        format_lalrpop_parse_error(&error)
                    ),
                    Some(span),
                );
                (parse_error, editor_facts)
            }
            Self::Db {
                message,
                mut editor_facts,
            } => {
                let parse_error =
                    Error::diagram_parse_fallback(diagram_type.to_string(), message.clone());
                editor_facts.mark_recovered_from_parse_error(
                    format!("sequence semantic construction failed: {message}"),
                    None,
                );
                (parse_error, editor_facts)
            }
        }
    }
}

pub(crate) fn parse_sequence(code: &str, meta: &ParseMetadata) -> Result<Value> {
    Ok(parse_sequence_semantic_source(code, meta)?
        .db
        .into_model(meta))
}

pub(crate) fn parse_sequence_model_for_render_controlled(
    code: &str,
    meta: &ParseMetadata,
    control: &OperationControl,
) -> OperationControlResult<Result<SequenceDiagramRenderModel>> {
    let construction =
        construct_sequence_semantic_source(code, sequence_wrap_enabled(meta), control)?;
    match construction {
        Ok(source) => {
            let mut model = source.db.into_render_model();
            if meta
                .effective_config
                .as_value()
                .get("traceSource")
                .and_then(Value::as_bool)
                != Some(true)
            {
                model.source_occurrences.clear();
            }
            Ok(Ok(model))
        }
        Err(failure) => Ok(Err((*failure).into_parse_error(meta, code.len()))),
    }
}

pub(crate) fn parse_sequence_json_and_editor_facts(
    code: &str,
    meta: &ParseMetadata,
    control: &OperationControl,
) -> OperationControlResult<crate::family::CombinedSemanticParse> {
    let construction =
        construct_sequence_semantic_source(code, sequence_wrap_enabled(meta), control)?;
    let parsed = crate::family::CombinedSemanticParse::from_construction(
        construction,
        |SequenceSemanticSource { db, editor_facts }| (Ok(db.into_model(meta)), editor_facts),
        |failure| (*failure).into_error_and_editor_facts(meta, code.len()),
    );
    control.checkpoint()?;
    Ok(parsed)
}

fn parse_sequence_semantic_source(
    code: &str,
    meta: &ParseMetadata,
) -> Result<SequenceSemanticSource> {
    construct_sequence_semantic_source(code, sequence_wrap_enabled(meta), &OperationControl::new())
        .expect("a private parse control cannot be cancelled")
        .map_err(|failure| (*failure).into_parse_error(meta, code.len()))
}

fn construct_sequence_semantic_source(
    code: &str,
    wrap_enabled: Option<bool>,
    control: &OperationControl,
) -> OperationControlResult<std::result::Result<SequenceSemanticSource, Box<SequenceSemanticFailure>>>
{
    let syntax = SequenceSyntax::lex(code, control)?;
    let (editor_facts, actions, events) = syntax.into_editor_facts_and_actions(code, control)?;
    let actions = match actions {
        Ok(actions) => actions,
        Err(error) => {
            return Ok(Err(Box::new(SequenceSemanticFailure::Grammar {
                error,
                editor_facts,
            })));
        }
    };

    let db = match build_sequence_db(actions, wrap_enabled, &events, code, control)? {
        Ok(db) => db,
        Err(message) => {
            return Ok(Err(Box::new(SequenceSemanticFailure::Db {
                message,
                editor_facts,
            })));
        }
    };

    control.checkpoint()?;
    Ok(Ok(SequenceSemanticSource { db, editor_facts }))
}

fn build_sequence_db(
    actions: Vec<super::Action>,
    wrap_enabled: Option<bool>,
    events: &[SequenceLexicalEvent],
    code: &str,
    control: &OperationControl,
) -> OperationControlResult<std::result::Result<SequenceDb, String>> {
    let mut db = SequenceDb::new(wrap_enabled);
    let tokens: Vec<_> = events
        .iter()
        .filter_map(|event| event.as_ref().ok())
        .collect();
    let mut activations: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, action) in actions.into_iter().enumerate() {
        if index % 128 == 0 {
            control.checkpoint()?;
        }
        let super::Action::Located { span, action } = action else {
            return Ok(Err("sequence action lost its source location".into()));
        };
        let first = tokens.partition_point(|(start, _, _)| *start < span.start);
        let last = tokens.partition_point(|(start, _, _)| *start < span.end);
        let within: Vec<_> = tokens[first..last]
            .iter()
            .copied()
            .filter(|(_, _, end)| *end <= span.end)
            .collect();
        let label = within.iter().find_map(|(start, token, end)| {
            let text = match token {
                Tok::Text(text) | Tok::RestOfLine(text) => text,
                _ => return None,
            };
            if matches!(action.as_ref(), super::Action::BoxStart { .. }) {
                return sequence_box_name_and_selection(text, *start, *end, code)
                    .map(|(_, span)| span);
            }
            let mut text = text.trim();
            for prefix in ["wrap:", "nowrap:", ":wrap:", ":nowrap:"] {
                if text
                    .get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
                {
                    text = text[prefix.len()..].trim();
                    break;
                }
            }
            sequence_payload_selection(text, *start, *end, code)
        });
        let actor = match action.as_ref() {
            super::Action::AddParticipant { id, .. }
            | super::Action::CreateParticipant { id, .. } => Some((id.clone(), true)),
            super::Action::EnsureParticipant { id }
                if !within
                    .first()
                    .is_some_and(|(_, token, _)| matches!(token, Tok::Note)) =>
            {
                Some((id.clone(), false))
            }
            _ => None,
        };
        let kind = match action.as_ref() {
            super::Action::AddMessage { .. } => "edge",
            super::Action::AddNote { .. } => "note",
            super::Action::ActiveStart { .. } | super::Action::ActiveEnd { .. } => "activation",
            super::Action::ControlSignal { .. } => "control",
            _ => "decoration",
        };
        let note = match action.as_ref() {
            super::Action::AddNote {
                actors, placement, ..
            } => Some((actors.clone(), *placement)),
            _ => None,
        };
        let first_message = db.messages.len();
        let box_key = matches!(action.as_ref(), super::Action::BoxStart { .. })
            .then(|| format!("box:{}", db.boxes.len()));
        let activation = match action.as_ref() {
            super::Action::ActiveStart { actor } => Some((actor.clone(), true)),
            super::Action::ActiveEnd { actor } => Some((actor.clone(), false)),
            _ => None,
        };
        let has_label = !matches!(
            action.as_ref(),
            super::Action::ControlSignal { text: None, .. }
                | super::Action::ActiveStart { .. }
                | super::Action::ActiveEnd { .. }
        );
        if let Err(error) = db.apply_controlled(*action, control)? {
            return Ok(Err(error));
        }
        if let Some((id, declaration)) = actor {
            let key = format!("actor:{id}");
            let previous = db.source_occurrences.iter().position(|p| p["domId"] == key);
            if declaration || previous.is_none() {
                let token_span = within.iter().find_map(|(start, token, end)| match token {
                    Tok::Actor(name) if name == &id => Some(SourceSpan::new(*start, *end)),
                    Tok::Num(value) if value.to_string() == id => {
                        Some(SourceSpan::new(*start, *end))
                    }
                    _ => None,
                });
                if let Some(token_span) = token_span {
                    let occurrence = serde_json::json!({"kind":"node", "semanticId": id, "domId":key,
                        "span":if declaration {span} else {token_span}, "labelSpan": if declaration {label.unwrap_or(token_span)} else {token_span}});
                    if let Some(index) = previous {
                        db.source_occurrences[index] = occurrence;
                    } else {
                        db.source_occurrences.push(occurrence);
                    }
                }
            }
        }
        if let Some(key) = box_key {
            let mut occurrence =
                serde_json::json!({"kind":"control", "semanticId":key,"domId":key,"span":span});
            if let Some(label) = label {
                occurrence["labelSpan"] = serde_json::to_value(label).unwrap();
            }
            db.source_occurrences.push(occurrence);
        }
        for message in &db.messages[first_message..] {
            let mut occurrence = serde_json::json!({"kind":kind,"semanticId":message.id,"domId":format!("message:{}",message.id),"span":span});
            if kind == "edge" {
                occurrence["from"] = serde_json::to_value(&message.from).unwrap();
                occurrence["to"] = serde_json::to_value(&message.to).unwrap();
            }
            if let Some((targets, placement)) = &note {
                occurrence["targets"] = serde_json::json!(targets);
                occurrence["placement"] = serde_json::json!(placement);
            }
            if has_label {
                if let Some(label) = label {
                    occurrence["labelSpan"] = serde_json::to_value(label).unwrap();
                }
            }
            db.source_occurrences.push(occurrence);
        }
        if let Some((actor, start)) = activation {
            let stack = activations.entry(actor).or_default();
            if start {
                stack.push(db.source_occurrences.len() - 1);
            } else if let Some(index) = stack.pop() {
                db.source_occurrences[index]["span"]["end"] = serde_json::json!(span.end);
            }
        }
    }
    let note_targets: Vec<String> = db
        .source_occurrences
        .iter()
        .filter(|piece| piece["kind"] == "note")
        .flat_map(|piece| piece["targets"].as_array().into_iter().flatten())
        .filter_map(|target| target.as_str().map(str::to_owned))
        .collect();
    for target in note_targets {
        let key = format!("actor:{target}");
        if !db
            .source_occurrences
            .iter()
            .any(|piece| piece["domId"] == key)
        {
            db.source_occurrences.push(serde_json::json!({"kind":"decoration", "semanticId":target, "domId":key, "classification":"note-implied-anchor"}));
        }
    }
    Ok(Ok(db))
}

fn sequence_wrap_enabled(meta: &ParseMetadata) -> Option<bool> {
    meta.effective_config
        .as_value()
        .get("wrap")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            meta.effective_config
                .as_value()
                .get("sequence")
                .and_then(|v| v.get("wrap"))
                .and_then(|v| v.as_bool())
        })
}

fn collect_sequence_editor_facts_from_events(
    events: &[SequenceLexicalEvent],
    code: &str,
    control: &OperationControl,
) -> OperationControlResult<EditorSemanticFacts> {
    let mut facts = EditorSemanticFacts::new();
    let mut collector = SequenceEditorFactCollector::default();

    for (index, event) in events.iter().enumerate() {
        if index % 128 == 0 {
            control.checkpoint()?;
        }
        match event {
            Ok((start, token, end)) => collector.accept(token, *start, *end, code, &mut facts),
            Err(_) => facts.mark_recovered(),
        }
    }

    control.checkpoint()?;
    Ok(facts)
}

#[derive(Debug, Default)]
struct SequenceEditorFactCollector {
    expected_actor: Option<ExpectedSequenceActor>,
    expected_text: Option<ExpectedSequenceText>,
    expected_rest_of_line: Option<ExpectedSequenceRestOfLine>,
    pending_message_source: Option<PendingSequenceActor>,
    participant_kinds: HashMap<String, EditorSemanticKind>,
}

#[derive(Debug)]
struct PendingSequenceActor {
    name: String,
    span: SourceSpan,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedSequenceActor {
    Participant,
    Actor,
    MessageTarget,
    NoteActor,
    EnsuredInteractionTarget,
    ReferenceOnly,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedSequenceText {
    Message,
    Note,
    Interaction,
    ParticipantLabel,
    FragmentLabel,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedSequenceRestOfLine {
    BoxLabel,
    Text(ExpectedSequenceText),
}

impl SequenceEditorFactCollector {
    fn accept(
        &mut self,
        token: &Tok,
        start: usize,
        end: usize,
        code: &str,
        facts: &mut EditorSemanticFacts,
    ) {
        match token {
            Tok::SequenceDiagram | Tok::Newline => self.reset_line_state(),
            Tok::Participant => self.expect_actor(ExpectedSequenceActor::Participant),
            Tok::ActorKw => self.expect_actor(ExpectedSequenceActor::Actor),
            Tok::Create => self.expect_actor(ExpectedSequenceActor::Participant),
            Tok::Destroy => self.expect_actor(ExpectedSequenceActor::ReferenceOnly),
            Tok::Activate | Tok::Deactivate => {
                self.expect_actor(ExpectedSequenceActor::ReferenceOnly)
            }
            Tok::Links => {
                facts.push_directive_prefix("links");
                self.expect_actor(ExpectedSequenceActor::EnsuredInteractionTarget);
            }
            Tok::Link => {
                facts.push_directive_prefix("link");
                self.expect_actor(ExpectedSequenceActor::EnsuredInteractionTarget);
            }
            Tok::Properties => {
                facts.push_directive_prefix("properties");
                self.expect_actor(ExpectedSequenceActor::EnsuredInteractionTarget);
            }
            Tok::Details => {
                facts.push_directive_prefix("details");
                self.expect_actor(ExpectedSequenceActor::EnsuredInteractionTarget);
            }
            Tok::Note => {
                self.pending_message_source = None;
            }
            Tok::LeftOf | Tok::RightOf | Tok::Over | Tok::Comma => {
                self.expect_actor(ExpectedSequenceActor::NoteActor);
            }
            Tok::Box => {
                self.expected_rest_of_line = Some(ExpectedSequenceRestOfLine::BoxLabel);
            }
            Tok::As => {
                self.expected_rest_of_line = Some(ExpectedSequenceRestOfLine::Text(
                    ExpectedSequenceText::ParticipantLabel,
                ));
            }
            Tok::Loop
            | Tok::Rect
            | Tok::Opt
            | Tok::Alt
            | Tok::Else
            | Tok::Par
            | Tok::ParOver
            | Tok::And
            | Tok::Critical
            | Tok::Option
            | Tok::Break => {
                self.expected_rest_of_line = Some(ExpectedSequenceRestOfLine::Text(
                    ExpectedSequenceText::FragmentLabel,
                ));
            }
            Tok::SignalType(_) => {
                self.push_pending_message_source(facts);
                self.expect_actor(ExpectedSequenceActor::MessageTarget);
            }
            Tok::Actor(name) => {
                if let Some(expected) = self.expected_actor {
                    self.push_actor(name.clone(), expected, start, end, facts);
                } else {
                    self.pending_message_source = Some(PendingSequenceActor {
                        name: name.clone(),
                        span: SourceSpan::new(start, end),
                    });
                }
            }
            Tok::RestOfLine(text) => match self.expected_rest_of_line.take() {
                Some(ExpectedSequenceRestOfLine::BoxLabel) => {
                    push_sequence_box_symbol(text.clone(), start, end, code, facts);
                }
                Some(ExpectedSequenceRestOfLine::Text(expected)) => {
                    push_sequence_text_payload(text.clone(), expected, start, end, code, facts);
                }
                None => {}
            },
            Tok::Text(text) => {
                if let Some(expected) = self.expected_text.take() {
                    push_sequence_text_payload(text.clone(), expected, start, end, code, facts);
                }
            }
            Tok::Title(text) | Tok::CompatTitle(text) => {
                facts.push_directive_prefix("title");
                push_sequence_named_payload(
                    text.clone(),
                    "sequence title",
                    start,
                    end,
                    code,
                    facts,
                );
            }
            Tok::AccTitle(text) => {
                facts.push_directive_prefix("accTitle");
                push_sequence_named_payload(
                    text.clone(),
                    "sequence accessibility title",
                    start,
                    end,
                    code,
                    facts,
                );
            }
            Tok::AccDescr(text) | Tok::AccDescrMultiline(text) => {
                facts.push_directive_prefix("accDescr");
                push_sequence_named_payload(
                    text.clone(),
                    "sequence accessibility description",
                    start,
                    end,
                    code,
                    facts,
                );
            }
            Tok::End
            | Tok::Autonumber
            | Tok::Off
            | Tok::Plus
            | Tok::Minus
            | Tok::Central
            | Tok::Num(_)
            | Tok::Config(_) => {}
        }
    }

    fn reset_line_state(&mut self) {
        self.expected_actor = None;
        self.expected_text = None;
        self.expected_rest_of_line = None;
        self.pending_message_source = None;
    }

    fn expect_actor(&mut self, expected: ExpectedSequenceActor) {
        self.expected_actor = Some(expected);
    }

    fn push_actor(
        &mut self,
        name: String,
        expected: ExpectedSequenceActor,
        start: usize,
        end: usize,
        facts: &mut EditorSemanticFacts,
    ) {
        let declared_kind = match expected {
            ExpectedSequenceActor::Actor => EditorSemanticKind::Variable,
            ExpectedSequenceActor::Participant
            | ExpectedSequenceActor::MessageTarget
            | ExpectedSequenceActor::NoteActor
            | ExpectedSequenceActor::EnsuredInteractionTarget
            | ExpectedSequenceActor::ReferenceOnly => EditorSemanticKind::Event,
        };
        let occurrence_detail = match expected {
            ExpectedSequenceActor::Actor => "sequence actor",
            ExpectedSequenceActor::Participant => "sequence participant",
            ExpectedSequenceActor::MessageTarget => "sequence participant reference",
            ExpectedSequenceActor::NoteActor => "sequence note participant",
            ExpectedSequenceActor::EnsuredInteractionTarget
            | ExpectedSequenceActor::ReferenceOnly => "sequence participant reference",
        };
        let span = SourceSpan::new(start, end);
        let is_explicit_declaration = matches!(
            expected,
            ExpectedSequenceActor::Participant | ExpectedSequenceActor::Actor
        );
        let ensures_participant = !matches!(expected, ExpectedSequenceActor::ReferenceOnly);
        let (is_definition, kind) = if is_explicit_declaration {
            let kind = *self
                .participant_kinds
                .entry(name.clone())
                .or_insert(declared_kind);
            (true, kind)
        } else if ensures_participant {
            match self.participant_kinds.entry(name.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(declared_kind);
                    (true, declared_kind)
                }
                Entry::Occupied(entry) => (false, *entry.get()),
            }
        } else {
            (
                false,
                self.participant_kinds
                    .get(&name)
                    .copied()
                    .unwrap_or(declared_kind),
            )
        };
        let detail = if is_definition && !is_explicit_declaration {
            "sequence implicit participant"
        } else {
            occurrence_detail
        };
        let symbol = if is_definition {
            EditorSemanticSymbol::new(name.clone(), Some(detail.to_string()), kind, span, span)
        } else {
            EditorSemanticSymbol::reference(
                name.clone(),
                Some(detail.to_string()),
                kind,
                span,
                span,
            )
        };
        facts.push_symbol(symbol);
        self.expected_text = match expected {
            ExpectedSequenceActor::MessageTarget => Some(ExpectedSequenceText::Message),
            ExpectedSequenceActor::NoteActor => Some(ExpectedSequenceText::Note),
            ExpectedSequenceActor::EnsuredInteractionTarget => {
                Some(ExpectedSequenceText::Interaction)
            }
            ExpectedSequenceActor::Participant
            | ExpectedSequenceActor::Actor
            | ExpectedSequenceActor::ReferenceOnly => None,
        };
        self.expected_actor = None;
    }

    fn push_pending_message_source(&mut self, facts: &mut EditorSemanticFacts) {
        if let Some(actor) = self.pending_message_source.take() {
            let (is_definition, kind) = match self.participant_kinds.entry(actor.name.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(EditorSemanticKind::Event);
                    (true, EditorSemanticKind::Event)
                }
                Entry::Occupied(entry) => (false, *entry.get()),
            };
            let symbol = if is_definition {
                EditorSemanticSymbol::new(
                    actor.name,
                    Some("sequence implicit participant".to_string()),
                    kind,
                    actor.span,
                    actor.span,
                )
            } else {
                EditorSemanticSymbol::reference(
                    actor.name,
                    Some("sequence participant reference".to_string()),
                    kind,
                    actor.span,
                    actor.span,
                )
            };
            facts.push_symbol(symbol);
        }
    }
}

fn push_sequence_text_payload(
    text: String,
    expected: ExpectedSequenceText,
    start: usize,
    end: usize,
    code: &str,
    facts: &mut EditorSemanticFacts,
) {
    let detail = match expected {
        ExpectedSequenceText::Message => "sequence message",
        ExpectedSequenceText::Note => "sequence note",
        ExpectedSequenceText::Interaction => "sequence interaction payload",
        ExpectedSequenceText::ParticipantLabel => "sequence participant label",
        ExpectedSequenceText::FragmentLabel => "sequence fragment label",
    };
    facts.push_expected_syntax(EditorExpectedSyntax::new(
        EditorExpectedSyntaxKind::Payload,
        SourceSpan::new(start, end),
    ));
    push_sequence_named_payload(text, detail, start, end, code, facts);
}

fn push_sequence_named_payload(
    text: String,
    detail: &str,
    start: usize,
    end: usize,
    code: &str,
    facts: &mut EditorSemanticFacts,
) {
    let Some(selection) = sequence_payload_selection(&text, start, end, code) else {
        return;
    };
    facts.push_symbol(EditorSemanticSymbol::payload(
        text,
        Some(detail.to_string()),
        EditorSemanticKind::String,
        SourceSpan::new(start, end),
        selection,
    ));
}

fn sequence_payload_selection(
    text: &str,
    start: usize,
    end: usize,
    code: &str,
) -> Option<SourceSpan> {
    if text.is_empty() {
        return None;
    }
    let raw = code.get(start..end)?;
    let local_start = raw.rfind(text)?;
    Some(SourceSpan::new(
        start + local_start,
        start + local_start + text.len(),
    ))
}

fn push_sequence_box_symbol(
    text: String,
    start: usize,
    end: usize,
    code: &str,
    facts: &mut EditorSemanticFacts,
) {
    facts.push_expected_syntax(EditorExpectedSyntax::new(
        EditorExpectedSyntaxKind::Payload,
        SourceSpan::new(start, end),
    ));
    let Some((name, selection)) = sequence_box_name_and_selection(&text, start, end, code) else {
        return;
    };
    facts.push_symbol(EditorSemanticSymbol::payload(
        name,
        Some("sequence box".to_string()),
        EditorSemanticKind::String,
        SourceSpan::new(start, end),
        selection,
    ));
}

fn sequence_box_name_and_selection(
    text: &str,
    start: usize,
    end: usize,
    code: &str,
) -> Option<(String, SourceSpan)> {
    let raw = code.get(start..end).unwrap_or(text);
    let leading = raw.len().saturating_sub(raw.trim_start().len());
    let trailing = raw.trim_end().len();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (color, title_candidate) = split_box_color_and_title(trimmed);
    let title_start = if is_css_color_value(color) {
        trimmed.len().saturating_sub(title_candidate.len())
    } else {
        0
    };
    let title = trimmed[title_start..].trim();
    if title.is_empty() {
        return None;
    }

    let local_start = leading + title_start + trimmed[title_start..].len()
        - trimmed[title_start..].trim_start().len();
    let local_end = start + trailing;
    Some((
        title.to_string(),
        SourceSpan::new(start + local_start, local_end),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorSemanticRole, MermaidConfig};

    fn meta() -> ParseMetadata {
        ParseMetadata {
            diagram_type: "sequence".to_string(),
            config: MermaidConfig::empty_object(),
            effective_config: MermaidConfig::empty_object(),
            title: None,
        }
    }

    fn editor_facts(source: &str) -> EditorSemanticFacts {
        crate::family::test_support::editor_facts(
            parse_sequence_json_and_editor_facts,
            source,
            &meta(),
        )
    }

    #[test]
    fn participant_definitions_references_and_payloads_have_distinct_roles() {
        let source = concat!(
            "sequenceDiagram\n",
            "actor Alice as User\n",
            "participant Bob\n",
            "Alice->>Bob: Request\n",
            "Note over Alice,Bob: Review\n",
            "activate Bob\n",
            "deactivate Bob\n",
            "destroy Bob\n",
        );
        let facts = editor_facts(source);

        let alice: Vec<_> = facts
            .symbols
            .iter()
            .filter(|symbol| symbol.name == "Alice")
            .collect();
        let bob: Vec<_> = facts
            .symbols
            .iter()
            .filter(|symbol| symbol.name == "Bob")
            .collect();
        assert_eq!(alice[0].role, EditorSemanticRole::Entity);
        assert_eq!(alice[0].kind, EditorSemanticKind::Variable);
        assert!(
            alice[1..]
                .iter()
                .all(|symbol| symbol.role == EditorSemanticRole::Reference)
        );
        assert!(
            alice[1..]
                .iter()
                .all(|symbol| symbol.kind == EditorSemanticKind::Variable)
        );
        assert_eq!(bob[0].role, EditorSemanticRole::Entity);
        assert!(
            bob[1..]
                .iter()
                .all(|symbol| symbol.role == EditorSemanticRole::Reference)
        );
        assert!(facts.symbols.iter().any(|symbol| {
            symbol.name == "Request" && symbol.role == EditorSemanticRole::Payload
        }));
        assert!(facts.symbols.iter().any(|symbol| {
            symbol.name == "Review" && symbol.role == EditorSemanticRole::Payload
        }));
    }

    #[test]
    fn unicode_implicit_participant_is_defined_once_then_referenced() {
        let source = concat!(
            "sequenceDiagram\n",
            "顾客->>服务: 创建\n",
            "服务-->>顾客: 完成\n",
        );
        let facts = editor_facts(source);

        for name in ["顾客", "服务"] {
            let occurrences: Vec<_> = facts
                .symbols
                .iter()
                .filter(|symbol| symbol.name == name)
                .collect();
            assert_eq!(occurrences.len(), 2);
            assert_eq!(occurrences[0].role, EditorSemanticRole::Entity);
            assert_eq!(occurrences[1].role, EditorSemanticRole::Reference);
            assert_eq!(occurrences[0].kind, occurrences[1].kind);
            assert_eq!(
                &source[occurrences[0].selection.start..occurrences[0].selection.end],
                name
            );
            assert_eq!(
                &source[occurrences[1].selection.start..occurrences[1].selection.end],
                name
            );
        }
    }

    #[test]
    fn reference_only_statements_do_not_create_participant_entities() {
        let source = concat!(
            "sequenceDiagram\n",
            "activate Missing\n",
            "deactivate Missing\n",
            "destroy Missing\n",
        );
        let facts = editor_facts(source);
        let missing: Vec<_> = facts
            .symbols
            .iter()
            .filter(|symbol| symbol.name == "Missing")
            .collect();

        assert_eq!(missing.len(), 3);
        assert!(
            missing
                .iter()
                .all(|symbol| symbol.role == EditorSemanticRole::Reference)
        );
    }
    #[test]
    fn note_attachments_keep_ownership_without_becoming_actor_origins() {
        for suffix in ["", "\nA->>B: message", "\nparticipant A\nparticipant B"] {
            let note = "note over A,B: payload";
            let source = format!("sequenceDiagram\n{note}{suffix}\n");
            let db = parse_sequence_semantic_source(&source, &meta()).unwrap().db;
            let end = "sequenceDiagram\n".len() + note.len();
            assert!(
                !db.source_occurrences
                    .iter()
                    .any(|p| p["kind"] == "node"
                        && p["span"]["start"].as_u64().unwrap() < end as u64)
            );
            let owner = db
                .source_occurrences
                .iter()
                .find(|p| p["kind"] == "note")
                .unwrap();
            assert_eq!(owner["targets"], serde_json::json!(["A", "B"]));
            assert_eq!(owner["placement"], 2);
            if suffix.is_empty() {
                assert_eq!(
                    db.source_occurrences
                        .iter()
                        .filter(|p| p["classification"] == "note-implied-anchor")
                        .count(),
                    2
                );
            } else {
                assert_eq!(
                    db.source_occurrences
                        .iter()
                        .filter(|p| p["kind"] == "node")
                        .count(),
                    2
                );
            }
        }
    }
}
