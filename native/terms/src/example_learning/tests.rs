use super::*;
use crate::example_scores::{Tags, Vectors};

#[test]
fn grouped_holdouts_exclude_clear_hidden_and_training_background() {
    let key = "fixture";
    let mut ordinary: Vec<_> = (0..2010).map(|n| format!("candidate-{n:04}")).collect();
    ordinary.sort_by_key(|id| sha256_fields([key.as_bytes(), id.as_bytes()]));
    // 上限以外的未裁决图才有资格出现在候选里。
    let target = ordinary.last().unwrap().clone();
    let masked = ordinary[0].clone();
    let mut uuids = ordinary;
    uuids.extend(["A", "A-copy", "B", "C", "D", "E", "clear", "no", "hidden"].map(str::to_owned));
    uuids.sort_unstable();
    let positives = ["A", "A-copy", "B", "C", "D", "E"];
    let entries: Vec<_> = uuids
        .iter()
        .map(|id| {
            let red =
                positives.contains(&id.as_str()) || id == &target || id == &masked || id == "clear" || id == "hidden";
            (u32::from(!red), 1.0)
        })
        .collect();
    let assets: Vec<_> = uuids
        .iter()
        .map(|id| Asset {
            uuid: id.clone(),
            group: matches!(id.as_str(), "A" | "A-copy").then(|| "example-group".to_owned()),
            visible: id != "hidden",
        })
        .collect();
    let vectors = Vectors::Tags(Tags { width: 2, offsets: (0..=uuids.len()).collect(), entries });
    let matrix = Matrix { uuids, vectors };
    let mut label = Label {
        verdicts: positives
            .into_iter()
            .map(|id| (id.to_owned(), Verdict::Yes))
            .chain([("clear".into(), Verdict::Clear), ("no".into(), Verdict::No)])
            .collect(),
    };
    let plan = make_plan(&matrix, key, &label, &assets, "feature-fixture");
    assert_eq!((plan.positives, plan.negatives), (5, 1));
    assert_eq!(plan.candidates.iter().map(|(id, _)| id).collect::<Vec<_>>(), [&target]);
    assert!(plan.cutoff.is_some());
    let repeated = make_plan(&matrix, key, &label, &assets, "feature-fixture");
    assert_eq!((repeated.inputs, repeated.candidates), (plan.inputs, plan.candidates));

    label.verdicts.insert("E".into(), Verdict::Clear);
    let removed = make_plan(&matrix, key, &label, &assets, "feature-fixture");
    assert_eq!((removed.positives, removed.negatives), (4, 1));
    assert!(removed.candidates.is_empty());
    assert!(removed.cutoff.is_none());
}
