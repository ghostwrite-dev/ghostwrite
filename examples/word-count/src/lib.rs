//! `:word-count` says how many words the file has, `:shout` makes what's selected
//! uppercase, and it says the count whenever a file is saved.

use ghostwrite_plugin_api::*;

struct WordCount;

fn count(context: &Context) -> String {
    format!("{} words", context.text.split_whitespace().count())
}

impl Plugin for WordCount {
    fn run(name: String, _args: Vec<String>, context: Context) -> Reply {
        match name.as_str() {
            "word-count" => Ok(vec![Action::Say(count(&context))]),
            "shout" => {
                let (start, end) = (context.selection_start, context.selection_end);
                let selected = &context.text[start as usize..end as usize];
                Ok(vec![Action::Edit(Edit { start, end, text: selected.to_uppercase() })])
            }
            other => Err(format!("no command {other}")),
        }
    }

    fn on_event(_event: Event, context: Context) -> Reply {
        Ok(vec![Action::Say(count(&context))])
    }
}

register!(WordCount);
