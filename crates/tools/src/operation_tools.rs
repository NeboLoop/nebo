//! Operation tools: one tool per interfaces-catalog operation that has a
//! provider right now (`ledger.bill.create` → `ledger_bill_create`). A
//! provider is whatever performs catalog operations: an installed plugin
//! that binds them, or the runtime itself. The tool is the operation; which
//! provider performs it is resolution, never a second tool. Its rule key is
//! the catalog operation itself (a rule on the tool's name matches it too),
//! and `operation_performed` names the same operation.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::origin::ToolContext;
use crate::plugin_tool::PluginRunner;
use crate::registry::{DynTool, ToolResult};

/// The catalog actions that only read. An operation whose last segment is
/// one of these, and which the catalog does not gate, changes nothing.
const READ_ACTIONS: &[&str] = &[
    "get", "list", "search", "find", "query", "read", "status", "stats", "history", "usage",
];

/// Input fields that name who money goes to or comes from.
const COUNTERPARTY_FIELDS: &[&str] = &["counterparty", "vendorId", "vendor", "customerId", "customer", "payee"];
/// Input fields that name who a message goes to.
const RECIPIENT_FIELDS: &[&str] = &["to", "sendTo", "recipient", "recipients", "cc", "bcc"];
/// Input fields that carry an amount in cents.
const AMOUNT_FIELDS: &[&str] = &["amountCents", "amount_cents"];

/// The tool an operation is called through: its segments joined by `_`
/// (`email-marketing.campaign.send` → `email_marketing_campaign_send`).
pub fn operation_tool_name(operation: &str) -> String {
    operation.to_lowercase().replace(['.', '-'], "_")
}

/// A performer of catalog operations.
pub trait OperationProvider: Send + Sync {
    /// Who performs them: a plugin's slug, or a built-in's name. It is the
    /// `provider` value when more than one binds an operation.
    fn provider(&self) -> &str;
    /// The owner-facing name of the service ("QuickBooks").
    fn service(&self) -> String;
    /// The operations it performs now, each with the input its call takes.
    fn operations(&self) -> Vec<ProvidedOperation>;
    /// Perform `operation` with the call's own input.
    fn perform<'a>(
        &'a self,
        ctx: &'a ToolContext,
        operation: &'a str,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;
}

/// One operation a provider performs, and the input it takes.
#[derive(Debug, Clone, Default)]
pub struct ProvidedOperation {
    /// The catalog operation, as the provider binds it (`ledger.bill.create`).
    pub operation: String,
    /// JSON Schema properties of the call's input.
    pub properties: Map<String, Value>,
    pub required: Vec<String>,
    /// Fields that carry an amount in cents.
    pub cents: Vec<String>,
    /// Fields the properties don't name are passed on too.
    pub open: bool,
    /// One line for the description: how this provider takes the call.
    pub note: String,
}

/// A provider and the operation it performs.
type Provided = (Arc<dyn OperationProvider>, ProvidedOperation);

/// A catalog operation as a tool, with every provider that performs it.
pub struct OperationTool {
    name: String,
    operation: String,
    /// The catalog term the operation belongs to (`ledger`): what an
    /// employee's `requires.interfaces` names.
    interface: String,
    hint: String,
    description: String,
    schema: Value,
    providers: Vec<Provided>,
}

/// One tool per operation the providers perform, in name order.
pub fn operation_tools(providers: &[Arc<dyn OperationProvider>]) -> Vec<OperationTool> {
    let mut by_op: BTreeMap<String, Vec<Provided>> = BTreeMap::new();
    for provider in providers {
        for op in provider.operations() {
            by_op
                .entry(op.operation.clone())
                .or_default()
                .push((provider.clone(), op));
        }
    }
    by_op
        .into_iter()
        .map(|(operation, mut providers)| {
            providers.sort_by(|a, b| a.0.provider().cmp(b.0.provider()));
            OperationTool::new(operation, providers)
        })
        .collect()
}

impl OperationTool {
    fn new(operation: String, providers: Vec<Provided>) -> Self {
        let name = operation_tool_name(&operation);
        let interface = operation.split('.').next().unwrap_or_default().to_string();
        let hint = operation.replace(['.', '-'], " ");
        let gated = crate::interface_catalog::is_gated(&operation);
        let read_only = reads_only(&operation);
        let schema = Self::build_schema(&providers, gated, read_only);
        let description = Self::describe(&operation, &providers, gated, read_only);
        Self {
            name,
            operation,
            interface,
            hint,
            description,
            schema,
            providers,
        }
    }

    fn describe(operation: &str, providers: &[Provided], gated: bool, read_only: bool) -> String {
        let mut out = match providers {
            [(p, _)] => format!("Performs {operation} through {}.\n", p.service()),
            _ => format!(
                "Performs {operation} through one of: {}. `provider` picks which.\n",
                providers
                    .iter()
                    .map(|(p, _)| p.provider())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        if gated {
            out.push_str(
                "- The owner's approval rules cover it. Write `display`: one plain sentence with \
                 real names and amounts, e.g. \"Pay Acme Supplies $2,500.00 for bill 1042\".\n",
            );
        }
        if !read_only {
            out.push_str(
                "- `clientKey` makes a write run once: the same call under the same key returns \
                 the first result.\n",
            );
        }
        for (_, op) in providers {
            if !op.note.is_empty() {
                out.push_str(&format!("- {}\n", op.note));
            }
        }
        out.trim_end().to_string()
    }

    fn build_schema(providers: &[Provided], gated: bool, read_only: bool) -> Value {
        let mut properties = Map::new();
        let mut required: Vec<String> = Vec::new();
        let mut open = false;
        if providers.len() > 1 {
            properties.insert(
                "provider".into(),
                serde_json::json!({
                    "type": "string",
                    "enum": providers.iter().map(|(p, _)| p.provider()).collect::<Vec<_>>(),
                    "description": "Which service performs it."
                }),
            );
            required.push("provider".into());
        }
        for (_, op) in providers {
            for (k, v) in &op.properties {
                properties.entry(k.clone()).or_insert_with(|| v.clone());
            }
            open |= op.open;
        }
        // With one provider its required fields are the tool's; with several
        // each provider says what it misses.
        if let [(_, op)] = providers {
            required.extend(op.required.iter().cloned());
        }
        if gated {
            properties.insert(
                "display".into(),
                serde_json::json!({
                    "type": "string",
                    "description": "One plain sentence the owner reads if this needs their approval."
                }),
            );
        }
        if !read_only {
            properties.insert(
                "clientKey".into(),
                serde_json::json!({
                    "type": "string",
                    "description": "Idempotency key: the same write under it runs once."
                }),
            );
        }
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": open,
        });
        if !required.is_empty() {
            schema["required"] = serde_json::json!(required);
        }
        schema
    }

    /// The provider this call names, or the only one.
    fn provider_for(&self, input: &Value) -> Result<&Provided, String> {
        if let [one] = self.providers.as_slice() {
            return Ok(one);
        }
        let named = input.get("provider").and_then(|p| p.as_str()).unwrap_or_default();
        self.providers
            .iter()
            .find(|(p, _)| p.provider() == named)
            .ok_or_else(|| {
                format!(
                    "`provider` must be one of: {}.",
                    self.providers
                        .iter()
                        .map(|(p, _)| p.provider())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}

/// An operation that reads: the catalog doesn't gate it and its action is a
/// read.
fn reads_only(operation: &str) -> bool {
    !crate::interface_catalog::is_gated(operation)
        && operation.rsplit('.').next().is_some_and(|a| READ_ACTIONS.contains(&a))
}

fn text_fields(input: &Value, fields: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for f in fields {
        match input.get(*f) {
            Some(Value::String(s)) if !s.trim().is_empty() => out.push(s.trim().to_string()),
            Some(Value::Array(items)) => out.extend(items.iter().filter_map(|i| i.as_str()).map(str::to_string)),
            Some(Value::Number(n)) => out.push(n.to_string()),
            _ => {}
        }
    }
    out
}

impl DynTool for OperationTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn search_hint(&self) -> &str {
        &self.hint
    }

    fn read_only(&self, _input: &Value) -> bool {
        reads_only(&self.operation)
    }

    fn rule_key(&self, _input: &Value) -> String {
        self.operation.clone()
    }

    // No job capability: an operation is decided by the rules written for
    // it (and the catalog's gating), as plugin operations always were. A
    // catalog term as its capability waits for the consent that grants an
    // employee its bound interfaces; until then every operation would sit
    // outside every job.

    fn operation_performed(&self, _input: &Value) -> Option<String> {
        Some(self.operation.clone())
    }

    /// The amount, who it goes to and who a message reaches, as far as the
    /// call's fields say; anything they don't say stays unknown.
    fn effects(&self, input: &Value) -> types::permissions::CallEffects {
        if self.read_only(input) {
            return types::permissions::CallEffects::none();
        }
        let mut effects = types::permissions::CallEffects::unknown();
        let cents: Vec<&str> = self
            .providers
            .iter()
            .flat_map(|(_, op)| op.cents.iter().map(String::as_str))
            .chain(AMOUNT_FIELDS.iter().copied())
            .collect();
        effects.money_cents = cents.iter().find_map(|f| match input.get(*f) {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        });
        effects.counterparty = text_fields(input, COUNTERPARTY_FIELDS).into_iter().next();
        effects.recipients = text_fields(input, RECIPIENT_FIELDS);
        // A customer send reaches only the people it names.
        if crate::effects::is_customer_send(&self.operation) {
            effects.publishes = types::permissions::Knowable::No;
        }
        // A delete names the record it removes the way the record's create
        // is recorded (`Check::ran`): by the operation's resource and id.
        if let Some((resource, "delete")) = crate::plugin_tool::port_suffix(&self.operation).rsplit_once('.')
            && let Some(id) = crate::plugin_tool::record_id(input)
        {
            effects.deletes.push(format!("{resource}:{id}"));
        }
        effects
    }

    fn taint(&self, input: &Value) -> Option<types::provenance::ProvenanceClass> {
        if !self.read_only(input) {
            return None;
        }
        match self.interface.as_str() {
            "mail" => Some(types::provenance::ProvenanceClass::ExternalEmail),
            "sms" => Some(types::provenance::ProvenanceClass::Channel),
            _ => None,
        }
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        self.provider_for(input).map(|_| ())
    }

    fn activity(&self, input: &Value) -> String {
        let (gerund, _) = self.labels();
        display(input).unwrap_or(gerund)
    }

    fn outcome(&self, input: &Value) -> String {
        let (_, past) = self.labels();
        display(input).unwrap_or(past)
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        mut input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let provider = match self.provider_for(&input) {
                Ok((p, _)) => p.clone(),
                Err(e) => return ToolResult::error(e),
            };
            if let Some(obj) = input.as_object_mut() {
                obj.remove("provider");
                obj.remove("display");
            }
            provider.perform(ctx, &self.operation, input).await
        })
    }
}

impl OperationTool {
    /// The catalog term the operation belongs to (`ledger`).
    pub fn interface(&self) -> &str {
        &self.interface
    }

    /// "creating bill in QuickBooks" / "Created bill in QuickBooks".
    fn labels(&self) -> (String, String) {
        let mut parts = self.operation.split('.').skip(1).collect::<Vec<_>>();
        let action = parts.pop().unwrap_or_default();
        let noun = parts.join(" ");
        let service = match self.providers.as_slice() {
            [(p, _)] => format!(" in {}", p.service()),
            _ => String::new(),
        };
        match crate::humanize::strap_verb(action) {
            Some((gerund, past)) => (format!("{gerund} {noun}{service}"), format!("{past} {noun}{service}")),
            None => (
                format!("running {action} on {noun}{service}"),
                format!("Ran {action} on {noun}{service}"),
            ),
        }
    }
}

/// The model's approval sentence, when it wrote one.
fn display(input: &Value) -> Option<String> {
    input
        .get("display")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

/// An installed, connected plugin as a provider of the operations its
/// manifest binds.
pub struct PluginProvider {
    slug: String,
    runner: Arc<PluginRunner>,
}

impl PluginProvider {
    pub fn new(runner: Arc<PluginRunner>, slug: &str) -> Self {
        Self {
            slug: slug.to_string(),
            runner,
        }
    }

    /// The input a binding template takes: each placeholder a parameter,
    /// described by the flag it fills.
    fn provided(&self, operation: &str, template: &str, skills: &[String]) -> ProvidedOperation {
        use napp::plugin::BindingPart;
        let mut op = ProvidedOperation {
            operation: operation.to_string(),
            open: true,
            ..Default::default()
        };
        let words = napp::plugin::parse_binding_template(template).unwrap_or_default();
        let mut flag: Option<String> = None;
        for parts in &words {
            for part in parts {
                let (name, schema, required) = match part {
                    BindingPart::Literal(_) => continue,
                    BindingPart::Field(name) => (name, serde_json::json!({"type": "string"}), true),
                    BindingPart::Cents(name) => {
                        op.cents.push(name.clone());
                        (
                            name,
                            serde_json::json!({"type": "integer", "description": "Amount in cents."}),
                            true,
                        )
                    }
                    BindingPart::List { field, flag: f } => (
                        field,
                        serde_json::json!({"type": "array", "items": {"type": "string"}, "description": format!("One {f} per item.")}),
                        true,
                    ),
                    BindingPart::Optional { field, flag: f } => (
                        field,
                        serde_json::json!({"type": "string", "description": format!("Sent as {f}.")}),
                        false,
                    ),
                };
                let mut schema = schema;
                if schema.get("description").is_none()
                    && let Some(f) = &flag
                {
                    schema["description"] = Value::String(format!("Sent as {f}."));
                }
                if required && !op.required.contains(name) {
                    op.required.push(name.clone());
                }
                op.properties.insert(name.clone(), schema);
            }
            flag = match parts.as_slice() {
                [BindingPart::Literal(w)] if w.starts_with("--") => Some(w.clone()),
                _ => None,
            };
        }
        let service = self.service();
        let resource = operation.split('.').rev().nth(1).unwrap_or_default();
        let mut relevant: Vec<&String> = skills
            .iter()
            .filter(|s| !resource.is_empty() && s.contains(resource))
            .collect();
        if relevant.is_empty() {
            relevant = skills.iter().take(5).collect();
        }
        op.note = if op.properties.is_empty() {
            format!("Its fields go to {service} as --name value flags")
        } else {
            format!("Other fields go to {service} as --name value flags")
        };
        if relevant.is_empty() {
            op.note.push('.');
        } else {
            op.note.push_str(&format!(
                "; the skills that document them: {}.",
                relevant.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        op
    }
}

impl OperationProvider for PluginProvider {
    fn provider(&self) -> &str {
        &self.slug
    }

    fn service(&self) -> String {
        self.runner
            .plugin_store()
            .get_manifest(&self.slug)
            .map(|m| m.name.trim().to_string())
            .filter(|n| !n.is_empty() && *n != self.slug)
            .unwrap_or_else(|| crate::humanize::service_name(&self.slug))
    }

    fn operations(&self) -> Vec<ProvidedOperation> {
        let Some(manifest) = self.runner.plugin_store().get_manifest(&self.slug) else {
            return Vec::new();
        };
        let skills: Vec<String> = self
            .runner
            .list_services(&self.slug)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        let mut ops: Vec<ProvidedOperation> = manifest
            .interface_bindings
            .iter()
            .map(|(op, template)| self.provided(op, template, &skills))
            .collect();
        ops.sort_by(|a, b| a.operation.cmp(&b.operation));
        ops
    }

    fn perform<'a>(
        &'a self,
        ctx: &'a ToolContext,
        operation: &'a str,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(self.runner.perform_operation(ctx, &self.slug, operation, input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A provider with fixed operations, recording what it was asked to do.
    struct Fixed {
        name: &'static str,
        ops: Vec<ProvidedOperation>,
        seen: Mutex<Vec<(String, Value)>>,
    }

    impl OperationProvider for Fixed {
        fn provider(&self) -> &str {
            self.name
        }
        fn service(&self) -> String {
            crate::humanize::service_name(self.name)
        }
        fn operations(&self) -> Vec<ProvidedOperation> {
            self.ops.clone()
        }
        fn perform<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            operation: &'a str,
            input: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            self.seen.lock().unwrap().push((operation.to_string(), input));
            Box::pin(async move { ToolResult::ok(format!("{} did {operation}", self.name)) })
        }
    }

    fn fixed(name: &'static str, ops: &[&str]) -> Arc<Fixed> {
        Arc::new(Fixed {
            name,
            ops: ops
                .iter()
                .map(|o| ProvidedOperation {
                    operation: o.to_string(),
                    open: true,
                    ..Default::default()
                })
                .collect(),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn tools(providers: Vec<Arc<dyn OperationProvider>>) -> Vec<OperationTool> {
        operation_tools(&providers)
    }

    #[test]
    fn an_operation_is_named_by_its_segments() {
        assert_eq!(operation_tool_name("ledger.bill.create"), "ledger_bill_create");
        assert_eq!(operation_tool_name("ledger.card.limit.set"), "ledger_card_limit_set");
        assert_eq!(
            operation_tool_name("email-marketing.campaign.test-send"),
            "email_marketing_campaign_test_send"
        );
    }

    /// The spec every operation tool answers: the rule key is the catalog
    /// operation, reads are read-only and parallel, a
    /// gated operation asks for the owner's sentence and never reads, and a
    /// four-segment operation is its own gated entry.
    #[test]
    fn the_spec_follows_the_catalog() {
        let p = fixed(
            "ledgerly",
            &["ledger.invoice.list", "ledger.bill.create", "ledger.card.limit.set"],
        );
        let all = tools(vec![p]);
        let get = |n: &str| all.iter().find(|t| t.name() == n).unwrap();
        let list = get("ledger_invoice_list");
        assert!(list.read_only(&json()) && list.concurrency_safe(&json()));
        assert_eq!(list.effects(&json()), types::permissions::CallEffects::none());
        assert!(
            list.schema()["properties"].get("clientKey").is_none()
                && list.schema()["properties"].get("display").is_none()
        );
        let bill = get("ledger_bill_create");
        assert!(!bill.read_only(&json()));
        assert_eq!(bill.rule_key(&json()), "ledger.bill.create");
        assert_eq!(bill.operation_performed(&json()).as_deref(), Some("ledger.bill.create"));
        assert_eq!(bill.interface(), "ledger");
        assert_eq!(
            bill.capability(&json()),
            None,
            "the operation's own rules and gating decide it"
        );
        assert!(
            bill.schema()["properties"].get("display").is_some()
                && bill.schema()["properties"].get("clientKey").is_some()
        );
        assert!(
            bill.description()
                .starts_with("Performs ledger.bill.create through Ledgerly."),
            "{}",
            bill.description()
        );
        assert_eq!(bill.search_hint(), "ledger bill create");
        assert!(
            crate::interface_catalog::is_gated("ledger.card.limit.set"),
            "four segments are an entry, not a port"
        );
        assert!(
            get("ledger_card_limit_set").schema()["properties"]
                .get("display")
                .is_some()
        );
    }

    /// Money, who it goes to and who a message reaches, read from the call.
    #[test]
    fn effects_come_from_the_calls_fields() {
        let p = Arc::new(Fixed {
            name: "ledgerly",
            ops: vec![ProvidedOperation {
                operation: "ledger.payment.apply".into(),
                cents: vec!["totalCents".into()],
                open: true,
                ..Default::default()
            }],
            seen: Mutex::new(Vec::new()),
        });
        let all = tools(vec![p]);
        let e =
            all[0].effects(&serde_json::json!({"totalCents": 125005, "customerId": "21", "sendTo": "ap@example.com"}));
        assert_eq!(e.money_cents, Some(125005));
        assert_eq!(e.counterparty.as_deref(), Some("21"));
        assert_eq!(e.recipients, vec!["ap@example.com".to_string()]);
        assert_eq!(all[0].taint(&json()), None);
    }

    /// A customer send reaches only who it names; a delete names its record
    /// the way its create is recorded.
    #[test]
    fn sends_name_their_reach_and_deletes_their_record() {
        let send = &tools(vec![fixed("hub-sms", &["sms.message.send"])])[0];
        let e = send.effects(&serde_json::json!({"to": "+1-555-0142", "text": "shipped"}));
        assert_eq!((e.recipients, e.publishes), (vec!["+1-555-0142".to_string()], types::permissions::Knowable::No));
        let delete = &tools(vec![fixed("ledgerly", &["accounting.ap.ledger.bill.delete"])])[0];
        assert_eq!(delete.effects(&serde_json::json!({"id": 42})).deletes, vec!["ledger.bill:42"]);
    }

    /// Two providers of one operation: `provider` is required and picks one;
    /// the model's `display` and `provider` never reach the provider.
    #[tokio::test]
    async fn two_providers_make_the_call_name_one() {
        let a = fixed("alpha-mail", &["mail.message.send"]);
        let b = fixed("zeta-mail", &["mail.message.send"]);
        let all = tools(vec![b.clone(), a.clone()]);
        assert_eq!(all.len(), 1);
        let send = &all[0];
        assert_eq!(send.schema()["required"], serde_json::json!(["provider"]));
        assert_eq!(
            send.schema()["properties"]["provider"]["enum"],
            serde_json::json!(["alpha-mail", "zeta-mail"])
        );
        assert!(
            send.validate_input(&json())
                .unwrap_err()
                .contains("alpha-mail, zeta-mail")
        );
        let ctx = ToolContext::default();
        let r = send
            .execute_dyn(
                &ctx,
                serde_json::json!({"provider": "zeta-mail", "to": "a@example.com", "display": "Email Ann"}),
            )
            .await;
        assert_eq!(r.content, "zeta-mail did mail.message.send");
        assert_eq!(b.seen.lock().unwrap()[0].1, serde_json::json!({"to": "a@example.com"}));
        assert!(a.seen.lock().unwrap().is_empty());
        assert_eq!(send.activity(&serde_json::json!({"display": "Email Ann"})), "Email Ann");
        assert_eq!(send.taint(&json()), None, "a send brings nothing in");
    }

    /// A binding template is the schema: each placeholder a parameter, the
    /// required ones required, cents an integer, a list an array, the flag
    /// it fills in its description; other fields still pass through.
    #[test]
    fn a_binding_template_is_the_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let ps = Arc::new(napp::plugin::PluginStore::new(
            tmp.path().join("p"),
            tmp.path().join("u"),
            None,
        ));
        let db = Arc::new(db::Store::new(tmp.path().join("t.db").to_str().unwrap()).unwrap());
        let provider = PluginProvider::new(Arc::new(PluginRunner::new(ps, db)), "ledgerly");
        let op = provider.provided(
            "ledger.payment.apply",
            "payment apply --customer-ref {customerId} {invoiceIds[]:--line} --total-amt {amountCents:cents->dollars} {memo?:--memo}",
            &["ledgerly-payment".to_string(), "ledgerly-bill".to_string()],
        );
        assert_eq!(op.required, ["customerId", "invoiceIds", "amountCents"]);
        assert_eq!(
            op.properties["customerId"],
            serde_json::json!({"type": "string", "description": "Sent as --customer-ref."})
        );
        assert_eq!(op.properties["invoiceIds"]["type"], "array");
        assert_eq!(op.properties["amountCents"]["type"], "integer");
        assert_eq!(op.properties["memo"]["description"], "Sent as --memo.");
        assert_eq!(op.cents, ["amountCents"]);
        assert!(op.open);
        assert_eq!(
            op.note,
            "Other fields go to Ledgerly as --name value flags; the skills that document them: ledgerly-payment."
        );
        let plain = provider.provided("mail.message.send", "send", &[]);
        assert!(plain.properties.is_empty() && plain.required.is_empty());
        assert_eq!(plain.note, "Its fields go to Ledgerly as --name value flags.");
    }

    fn json() -> Value {
        serde_json::json!({})
    }
}
