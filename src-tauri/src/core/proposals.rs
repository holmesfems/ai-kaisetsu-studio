//! 推敲案の採用・不採用。

use super::Core;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

impl Core {
    /// 推敲案を採用・不採用にする。idx が None なら、その案の保留中の項目を全部。
    pub fn resolve_proposal(&self, pid: u64, idx: Option<usize>, accept: bool) -> Result<Value> {
        // この推敲案だけを写して直し、最後にこの推敲案だけを書き戻す（丸ごと戻すと、その間に Claude が出した推敲案が消える）
        let mut p = self.hub.proposals.lock().iter().find(|p| p.id == pid).cloned()
            .ok_or_else(|| anyhow!("推敲案が見つかりません"))?;
        let p = &mut p;
        let targets: Vec<usize> = match idx {
            Some(i) => vec![i],
            None => (0..p.items.len()).filter(|&i| p.items[i].status == "pending").collect(),
        };
        let mut script = self.script().ok_or_else(|| anyhow!(crate::msg::NO_SCRIPT))?;
        let mut errors = vec![];
        for i in targets {
            let earlier = p.items[..i.min(p.items.len())].to_vec();
            let Some(it) = p.items.get_mut(i) else { continue };
            if it.status != "pending" {
                continue;
            }
            if !accept {
                it.status = "rejected".into();
                continue;
            }
            match crate::script::ops::apply_item(&mut script, it, &earlier) {
                Ok(()) => it.status = "accepted".into(),
                Err(e) => {
                    it.status = "stale".into();
                    errors.push(format!("{e:#}"));
                }
            }
        }
        let summary = json!({"id": p.id, "items": p.items.iter().map(|i| &i.status).collect::<Vec<_>>()});
        if let Some(slot) = self.hub.proposals.lock().iter_mut().find(|x| x.id == pid) {
            *slot = p.clone();
        }
        self.hub.save_proposals();
        if accept {
            let r = self.save_script(script)?;
            self.emit("script", json!({"script": r["script"], "external": false}));
        }
        self.emit("proposals", serde_json::to_value(&*self.hub.proposals.lock())?);
        Ok(json!({"summary": summary, "errors": errors}))
    }
}
