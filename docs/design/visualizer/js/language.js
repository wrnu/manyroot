// The language sheet: draws the knots and the palettes from the same code and tokens the views use.
"use strict";

(() => {
  const B = Bracken;
  document.querySelector(".masthead nav").append(B.themeButton());

  const KNOTS = [
    ["claim", "seed", "An idea, requirement, decision or observation: something that may grow.", { status: "pending", agent: 1 }],
    ["artifact", "nodule", "Something made, such as code or a document, swelling on the root.", { status: "pending", agent: 2 }],
    ["relation", "fork", "Where one strand reaches for another: supports, contradicts, implements and the rest.", { agent: 4 }],
    ["evidence", "spore", "A check's outcome: a filled spore for a pass, a struck ring for a fail.", { verdict: "pass" }],
    ["retract", "cut", "A pruning cut: the author withdrew something of their own.", { agent: 3 }],
  ];
  const STATES = [
    ["pending", "Hollow, in its author's ink: not enough counted passes yet."],
    ["accepted", "Filled green inside a soft bloom: the policy accepts it."],
    ["rejected", "Filled rust with a notch: a counted validator failed it."],
    ["superseded", "A dashed husk: an accepted replacement took its place."],
    ["retracted", "Dashed grey and cut through: withdrawn."],
  ];

  function render() {
    const knots = document.getElementById("knots");
    knots.replaceChildren(
      ...KNOTS.map(([kind, name, meaning, opts]) =>
        B.h("div", { class: "knot" }, B.knotIcon(kind, B.look(kind, opts), 44), B.h("span", { class: "name" }, `${B.KIND_WORD[kind]}: ${name}`), B.h("span", { class: "meaning" }, meaning))
      ),
      B.h("div", { class: "knot" }, B.knotIcon("evidence", B.look("evidence", { verdict: "fail" }), 44), B.h("span", { class: "name" }, "evidence: fail"), B.h("span", { class: "meaning" }, "Struck through, so a fail doesn't depend on colour."))
    );
    const states = document.getElementById("states");
    states.replaceChildren(
      ...STATES.map(([status, meaning]) =>
        B.h("div", { class: "knot" }, B.knotIcon("claim", B.look("claim", { status: status === "retracted" ? "pending" : status, retracted: status === "retracted", agent: 0 }), 44), B.h("span", { class: "name" }, status), B.h("span", { class: "meaning" }, meaning))
      )
    );
    for (const pal of document.querySelectorAll(".palette")) {
      const style = getComputedStyle(pal);
      const v = (n) => style.getPropertyValue(n).trim();
      const roles = [["--ground", "ground"], ["--ground-raised", "raised"], ["--stratum-line", "stratum"], ["--root", "text"], ["--root-dim", "secondary"], ["--accepted", "accepted"], ["--rejected", "rejected"], ["--focus", "focus, intents"]];
      pal.querySelector("[data-swatches]").replaceChildren(
        ...roles.map(([n, label]) => {
          const chip = B.h("span", { class: "chip" });
          chip.style.background = `var(${n})`;
          return B.h("span", { class: "swatch" }, chip, B.h("span", {}, label), B.h("span", { class: "val num" }, v(n)));
        })
      );
      const names = ["pollen", "iris", "frond", "bracken", "forget-me-not", "heather", "moss", "rosehip", "birch", "elderberry", "saffron", "sedge"];
      pal.querySelector("[data-inks]").replaceChildren(
        ...names.map((name, i) => {
          const strand = B.h("span", { class: "strand" });
          strand.style.background = `var(--ink-${i})`;
          return B.h("span", { class: "ink" }, strand, name);
        })
      );
    }
  }
  document.addEventListener("bracken-theme", render);
  render();
})();
