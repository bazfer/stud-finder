//! End-to-end checks on small inline Ruby fixtures.
//!
//! These lock in the first-slice extractor's behavior on the four reference
//! kinds we care about this slice: bare constants, constant paths, Rails
//! associations (singular + plural), and `class_name:` literal overrides.

use sf_ruby::{RefKind, RubyRef, extract_refs};
use std::path::Path;

fn names(refs: &[RubyRef], kind: RefKind) -> Vec<&str> {
    refs.iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.name.as_str())
        .collect()
}

#[test]
fn extracts_bare_constants() {
    let src = r#"
        class Order
          def process
            Payment.capture
            Notifier.send(Mailer, user)
          end
        end
    "#;
    let refs = extract_refs(Path::new("app/models/order.rb"), src);
    let constants = names(&refs, RefKind::Constant);
    assert!(constants.contains(&"Payment"), "got {constants:?}");
    assert!(constants.contains(&"Notifier"), "got {constants:?}");
    assert!(constants.contains(&"Mailer"), "got {constants:?}");
}

#[test]
fn extracts_constant_paths() {
    let src = r#"
        Billing::Invoice.find(id)
        Admin::Reports::Daily.generate
    "#;
    let refs = extract_refs(Path::new("app/services/thing.rb"), src);
    let constants = names(&refs, RefKind::Constant);
    assert!(constants.contains(&"Billing::Invoice"), "got {constants:?}");
    assert!(
        constants.contains(&"Admin::Reports::Daily"),
        "got {constants:?}"
    );
}

#[test]
fn extracts_belongs_to_singular() {
    let src = r#"
        class Post
          belongs_to :user
        end
    "#;
    let refs = extract_refs(Path::new("app/models/post.rb"), src);
    let assocs = names(&refs, RefKind::RailsAssociation);
    assert_eq!(assocs, vec!["User"]);
}

#[test]
fn extracts_has_many_plural_inflects() {
    let src = r#"
        class User
          has_many :comments
          has_many :user_roles
        end
    "#;
    let refs = extract_refs(Path::new("app/models/user.rb"), src);
    let assocs = names(&refs, RefKind::RailsAssociation);
    assert!(assocs.contains(&"Comment"), "got {assocs:?}");
    assert!(assocs.contains(&"UserRole"), "got {assocs:?}");
}

#[test]
fn extracts_has_and_belongs_to_many() {
    let src = r#"
        class Post
          has_and_belongs_to_many :tags
        end
    "#;
    let refs = extract_refs(Path::new("app/models/post.rb"), src);
    let assocs = names(&refs, RefKind::RailsAssociation);
    assert_eq!(assocs, vec!["Tag"]);
}

#[test]
fn class_name_option_overrides_name() {
    let src = r#"
        class Post
          belongs_to :author, class_name: "User"
          has_many :approvers, class_name: "Admin::Reviewer"
        end
    "#;
    let refs = extract_refs(Path::new("app/models/post.rb"), src);
    let overrides = names(&refs, RefKind::ClassNameOption);
    assert!(overrides.contains(&"User"), "got {overrides:?}");
    assert!(overrides.contains(&"Admin::Reviewer"), "got {overrides:?}");
}

#[test]
fn class_name_suppresses_inferred_association_name() {
    // Ohm P1: when `class_name:` is present on an association, the
    // inferred name from the symbol (e.g. `Author`) must NOT be emitted —
    // only the explicit class is a real reference.
    let src = r#"
        class Post
          belongs_to :author, class_name: "User"
          has_many :approvers, class_name: "Admin::Reviewer"
        end
    "#;
    let refs = extract_refs(Path::new("app/models/post.rb"), src);
    let assocs = names(&refs, RefKind::RailsAssociation);
    assert!(
        !assocs.contains(&"Author"),
        "class_name must suppress inferred `Author`; got {assocs:?}"
    );
    assert!(
        !assocs.contains(&"Approver"),
        "class_name must suppress inferred `Approver`; got {assocs:?}"
    );
}

#[test]
fn association_dsl_requires_receiverless_call() {
    // Ohm P2: `some_object.has_many(:users)` is a method call on an
    // unrelated receiver, not a Rails association declaration.
    let src = r#"
        class Thing
          def wire
            registry.belongs_to(:tenant)
            router.has_many(:routes)
            @mapper.has_one(:source)
            Builder.has_and_belongs_to_many(:scopes)
          end
        end
    "#;
    let refs = extract_refs(Path::new("app/services/thing.rb"), src);
    let assocs = names(&refs, RefKind::RailsAssociation);
    assert!(
        assocs.is_empty(),
        "no Rails associations should be emitted from receivered calls; got {assocs:?}"
    );
    let overrides = names(&refs, RefKind::ClassNameOption);
    assert!(
        overrides.is_empty(),
        "no class_name options should be emitted from receivered calls; got {overrides:?}"
    );
}

#[test]
fn self_receiver_still_counts_as_receiverless() {
    // `self.belongs_to :foo` is the same as `belongs_to :foo` at the top
    // of a class body — should still emit the association.
    let src = r#"
        class Post
          self.belongs_to :user
        end
    "#;
    let refs = extract_refs(Path::new("app/models/post.rb"), src);
    let assocs = names(&refs, RefKind::RailsAssociation);
    assert!(assocs.contains(&"User"), "got {assocs:?}");
}

#[test]
fn source_with_no_refs_emits_empty() {
    let src = r#"
        # Just a comment file.
        x = 1 + 2
        puts x
    "#;
    let refs = extract_refs(Path::new("lib/noop.rb"), src);
    assert!(
        refs.is_empty(),
        "expected no refs, got {:?}",
        refs.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
}

#[test]
fn line_numbers_are_populated() {
    let src = "class Order\n  belongs_to :user\nend\n";
    let refs = extract_refs(Path::new("app/models/order.rb"), src);
    let belongs = refs
        .iter()
        .find(|r| r.kind == RefKind::RailsAssociation && r.name == "User")
        .expect("expected User association ref");
    assert_eq!(belongs.line, 2);
}
