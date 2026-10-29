//! The human's channel and identity administration. Every channel change
//! and the message explaining it commit together.

use super::*;

#[derive(Clone, Debug)]
pub enum ChannelChange {
    SetOpen(bool),
    AddMember(String),
    RemoveMember(String),
    Hide,
}

impl Rules {
    pub async fn change_channel(
        &self,
        actor: Actor,
        channel_id: String,
        change: ChannelChange,
    ) -> Result<()> {
        let rules = self.clone();
        let now = self.now_text();
        let notice = self
            .db()
            .write(move |store| {
                // Use the shared access decision even when an agent attempts an
                // operator-only action, so closed channels remain undiscoverable.
                let channel = changeable_channel(store, &actor, &channel_id)?;
                human_only(&actor)?;
                let body = match change {
                    ChannelChange::SetOpen(open) => {
                        if channel.open == open {
                            return Ok(None);
                        }
                        store.set_channel_open(&channel_id, open)?;
                        format!(
                            "The operator made the channel {}.",
                            if open { "open" } else { "closed" }
                        )
                    }
                    ChannelChange::AddMember(name) => {
                        let name = name.trim();
                        if !agent_exists(store, name)? {
                            return Err(RuleError::agent_not_found(name));
                        }
                        if store.is_member(&channel_id, name)? {
                            return Ok(None);
                        }
                        store.add_member(&channel_id, name, &now)?;
                        format!("The operator added {name} to the channel.")
                    }
                    ChannelChange::RemoveMember(name) => {
                        let name = name.trim();
                        if !agent_exists(store, name)? {
                            return Err(RuleError::agent_not_found(name));
                        }
                        if !store.is_member(&channel_id, name)? {
                            return Ok(None);
                        }
                        store.remove_member(&channel_id, name)?;
                        format!("The operator removed {name} from the channel.")
                    }
                    ChannelChange::Hide => {
                        let notice = rules.post_as_operator(
                            store,
                            &channel,
                            "The operator hid the channel. Its messages are kept.".into(),
                        )?;
                        store.hide_channel(&channel_id)?;
                        return Ok(Some(notice));
                    }
                };
                Ok(Some(rules.post_as_operator(store, &channel, body)?))
            })
            .await?;
        if let Some(notice) = notice {
            self.publish_message(&notice);
        }
        Ok(())
    }

    pub async fn hide_agent(&self, actor: Actor, name: String) -> Result<()> {
        human_only(&actor)?;
        let body =
            format!("The operator hid {name} and revoked its access. Its messages are kept.");
        self.remove_identity(name, names::OPERATOR.to_string(), body)
            .await
    }

    /// Removes `me`'s own identity, as the operator's hide does: its token
    /// stops working, it cannot be reclaimed, it leaves every channel and
    /// drops out of agent lists. It says so itself in each channel it was in.
    /// Its messages are kept.
    pub async fn unregister(&self, me: &Agent) -> Result<()> {
        let body = format!(
            "{} unregistered and left the channel. Its messages are kept.",
            me.name
        );
        self.remove_identity(me.name.clone(), me.name.clone(), body)
            .await
    }

    /// Hides and revokes `name`, removing its memberships. Before that,
    /// `author` posts `body` in each channel `name` was in, in the same write.
    async fn remove_identity(&self, name: String, author: String, body: String) -> Result<()> {
        let rules = self.clone();
        let now = self.now_text();
        let notices = self
            .db()
            .write(move |store| {
                if !agent_exists(store, &name)? {
                    return Err(RuleError::agent_not_found(&name));
                }
                let mut notices = Vec::new();
                for id in store.all_channel_ids(Some(&name))? {
                    let channel = changeable_channel(store, &Actor::Human, &id)?;
                    notices.push(rules.post_notice(store, &channel, &author, body.clone())?);
                }
                store.hide_agent(&name, &now)?;
                Ok(notices)
            })
            .await?;
        for notice in notices {
            self.publish_message(&notice);
        }
        Ok(())
    }
}

pub(super) fn human_only(actor: &Actor) -> Result<()> {
    if matches!(actor, Actor::Human) {
        Ok(())
    } else {
        Err(RuleError::new(
            Code::Unauthenticated,
            "only the human operator may administer channels and identities",
        ))
    }
}
