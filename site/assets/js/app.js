"use strict";

(() => {
  const prefersReducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const liveAnnouncer = document.querySelector("[data-live-status]");

  function announce(message) {
    if (!liveAnnouncer) return;
    liveAnnouncer.textContent = "";
    window.requestAnimationFrame(() => {
      liveAnnouncer.textContent = message;
    });
  }

  /* 1. Responsive Popover Navbar (ResizeObserver-driven, zero fixed breakpoints) */
  function initNavbar() {
    const header = document.querySelector(".site-header");
    if (!header) return;

    const inner = header.querySelector(".header-inner");
    const menu = header.querySelector(".site-menu");
    const menuBtn = header.querySelector(".menu-button");
    if (!inner || !menu || !menuBtn) return;

    const supportsNativePopover = Object.hasOwn(HTMLElement.prototype, "popover");
    let lastHeaderWidth = header.getBoundingClientRect().width;

    function isMenuOpen() {
      return supportsNativePopover
        ? menu.matches(":popover-open")
        : menu.classList.contains(":popover-open");
    }

    function openMenu() {
      if (supportsNativePopover) {
        if (!menu.matches(":popover-open")) menu.showPopover();
      } else {
        menu.classList.add(":popover-open");
      }
    }

    function closeMenu() {
      if (supportsNativePopover) {
        if (menu.matches(":popover-open")) menu.hidePopover();
      } else {
        menu.classList.remove(":popover-open");
      }
    }

    function updateNavLayout(forceMeasure) {
      const currentWidth = header.getBoundingClientRect().width;
      const wasWide = header.classList.contains("is-wide");
      if (!forceMeasure && !wasWide && currentWidth <= lastHeaderWidth) {
        lastHeaderWidth = currentWidth;
        return;
      }
      lastHeaderWidth = currentWidth;

      const wasOpen = isMenuOpen();
      header.classList.add("is-measuring", "is-wide");
      menu.removeAttribute("popover");
      menu.classList.remove(":popover-open");

      const fitsInline = inner.scrollWidth <= inner.clientWidth + 1;

      if (fitsInline) {
        header.classList.add("is-wide");
        menu.removeAttribute("popover");
      } else {
        header.classList.remove("is-wide");
        menu.setAttribute("popover", "auto");
        if (wasOpen) {
          openMenu();
        }
      }

      window.requestAnimationFrame(() => {
        header.classList.remove("is-measuring");
      });
    }

    if (!supportsNativePopover) {
      menuBtn.addEventListener("click", () => {
        if (isMenuOpen()) closeMenu();
        else openMenu();
      });

      document.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && isMenuOpen()) {
          closeMenu();
          menuBtn.focus();
        }
      });

      document.addEventListener("click", (event) => {
        if (isMenuOpen() && !menu.contains(event.target) && !menuBtn.contains(event.target)) {
          closeMenu();
        }
      });
    }

    menu.addEventListener("click", (event) => {
      const link = event.target.closest("a");
      if (link && !header.classList.contains("is-wide")) {
        closeMenu();
      }
    });

    if ("ResizeObserver" in window) {
      const ro = new ResizeObserver((entries) => {
        for (const entry of entries) {
          updateNavLayout(entry.target === menu || header.classList.contains("is-wide"));
        }
      });
      ro.observe(header);
      ro.observe(menu);
    } else {
      window.addEventListener("resize", () => updateNavLayout(true));
    }

    if (document.fonts && document.fonts.ready) {
      document.fonts.ready.then(() => updateNavLayout(true));
    }
    updateNavLayout(true);
  }

  /* 1b. Smooth Animated Anchor Scrolling */
  function initSmoothAnchors() {
    let scrollRaf = 0;

    function cancelScroll() {
      if (scrollRaf) {
        window.cancelAnimationFrame(scrollRaf);
        scrollRaf = 0;
      }
    }

    window.addEventListener("wheel", cancelScroll, { passive: true });
    window.addEventListener("touchstart", cancelScroll, { passive: true });

    document.addEventListener("click", (event) => {
      const link = event.target.closest('a[href^="#"]');
      if (!link) return;

      const hash = link.getAttribute("href");
      if (!hash || hash === "#") return;

      const target = document.getElementById(hash.slice(1));
      if (!target) return;

      event.preventDefault();
      cancelScroll();

      const header = document.querySelector(".site-header");
      const headerHeight = header ? header.getBoundingClientRect().height : 56;
      const maxScroll = Math.max(0, document.documentElement.scrollHeight - window.innerHeight);
      const targetY = Math.min(
        maxScroll,
        Math.max(0, target.getBoundingClientRect().top + window.scrollY - headerHeight - 24)
      );

      if (window.history && window.history.pushState) {
        window.history.pushState(null, "", hash);
      }

      if (prefersReducedMotion) {
        window.scrollTo({ top: targetY, behavior: "instant" });
        return;
      }

      const startY = window.scrollY;
      const distance = targetY - startY;
      if (Math.abs(distance) < 2) return;

      const duration = Math.min(720, Math.max(460, Math.abs(distance) * 0.35));
      const startTime = performance.now();

      function step(now) {
        const elapsed = now - startTime;
        const progress = Math.min(1, elapsed / duration);
        const eased = 1 - Math.pow(1 - progress, 3);
        window.scrollTo({ top: startY + distance * eased, behavior: "instant" });
        if (progress < 1) {
          scrollRaf = window.requestAnimationFrame(step);
        } else {
          scrollRaf = 0;
        }
      }

      scrollRaf = window.requestAnimationFrame(step);
    });
  }

  /* 2. OS Auto-Detection & Download Switcher */
  function initPlatformDownload() {
    const labelNodes = document.querySelectorAll("[data-download-label]");
    const artifactNodes = document.querySelectorAll("[data-download-artifact]");
    const osButtons = document.querySelectorAll("[data-os-switch]");
    const heroInstallLabel = document.querySelector("[data-hero-install-label]");
    const heroInstallCmd = document.querySelector("[data-hero-install-cmd]");
    if (!labelNodes.length) return;

    const specs = {
      windows: {
        label: "Download for Windows (.exe)",
        artifact: "SquadAir-Setup-windows-x86_64.exe · Standalone installer · Windows 10 & 11",
        installLabel: "Install with Scoop (Windows):",
        installCmd: "scoop bucket add squadair https://github.com/alivergg/scoop-bucket && scoop install squadair/squadair",
        name: "Windows",
      },
      macos: {
        label: "Download for macOS (.pkg)",
        artifact: "SquadAir-Installer-macos-arm64.pkg · Apple Silicon installer · macOS 13+",
        installLabel: "Install with Homebrew (macOS arm64):",
        installCmd: "brew install --cask alivergg/tap/squadair",
        name: "macOS",
      },
      linux: {
        label: "Download for Linux",
        artifact: "Snap Store (squad-air) & native .deb / .rpm / Arch / .tar.gz · Linux x86_64",
        installLabel: "Install from Snap Store (Linux):",
        installCmd: "sudo snap install squad-air",
        name: "Linux",
      },
      android: {
        label: "Download for Android (.apk)",
        artifact: "SquadAir-android-arm64.apk · Signed arm64-v8a APK · Android 8.0+",
        installLabel: "Sideload via ADB (Android arm64-v8a):",
        installCmd: "adb install -r SquadAir-android-arm64.apk",
        name: "Android",
      },
    };

    function detectPlatform() {
      const uaDataPlatform = (navigator.userAgentData?.platform || "").toLowerCase();
      const ua = (navigator.userAgent || "").toLowerCase();
      const navPlatform = (navigator.platform || "").toLowerCase();
      if (uaDataPlatform.includes("android") || ua.includes("android") || navPlatform.includes("android")) {
        return "android";
      }
      const platform = uaDataPlatform || navPlatform || ua;
      if (platform.includes("mac") || platform.includes("darwin")) return "macos";
      if (platform.includes("linux")) return "linux";
      return "windows";
    }

    function applyPlatform(key, notify) {
      const spec = specs[key] || specs.windows;
      for (const el of labelNodes) {
        el.textContent = spec.label;
      }
      for (const el of artifactNodes) {
        el.textContent = spec.artifact;
      }
      if (heroInstallLabel) {
        heroInstallLabel.textContent = spec.installLabel;
      }
      if (heroInstallCmd) {
        heroInstallCmd.textContent = spec.installCmd;
      }
      for (const btn of osButtons) {
        btn.setAttribute("aria-pressed", String(btn.dataset.osSwitch === key));
      }
      if (notify) {
        announce(`Selected ${spec.name} download target`);
      }
    }

    for (const btn of osButtons) {
      btn.addEventListener("click", () => {
        applyPlatform(btn.dataset.osSwitch, true);
      });
    }

    applyPlatform(detectPlatform(), false);
  }

  /* 2b. Declarative Copy Buttons (Invoker Commands + Fallback) */
  function initCopyButtons() {
    const timers = new WeakMap();
    let lastMessage = "";

    function legacyCopy(text) {
      const field = Object.assign(document.createElement("textarea"), { value: text });
      field.setAttribute("readonly", "");
      field.style.cssText = "position:fixed;inset-block-start:0;opacity:0";
      document.body.append(field);
      field.select();
      try {
        return document.execCommand("copy");
      } finally {
        field.remove();
      }
    }

    function report(button, ok) {
      const tip = button.nextElementSibling;
      window.clearTimeout(timers.get(button));
      button.removeAttribute("data-copied");
      button.removeAttribute("data-failed");

      let message = ok ? "Copied" : "Copy failed";
      if (message === lastMessage) {
        message += "\u00A0";
      }
      lastMessage = message;

      if (tip) {
        tip.textContent = "";
      }
      void button.offsetWidth;
      button.setAttribute(ok ? "data-copied" : "data-failed", "");
      if (tip) {
        window.requestAnimationFrame(() => {
          tip.textContent = message;
        });
      }

      timers.set(
        button,
        window.setTimeout(() => {
          button.removeAttribute("data-copied");
          button.removeAttribute("data-failed");
          if (tip) {
            tip.textContent = "";
          }
        }, 1800)
      );
    }

    async function copyTarget(button, target) {
      if (!button || !target) return;
      const text = target.matches("input, textarea")
        ? target.value.trim()
        : target.innerText.trim();
      if (!text) return;
      try {
        if (navigator.clipboard && navigator.clipboard.writeText) {
          await navigator.clipboard.writeText(text);
        } else if (!legacyCopy(text)) {
          throw new Error("Copy failed");
        }
        report(button, true);
      } catch {
        try {
          if (legacyCopy(text)) {
            report(button, true);
            return;
          }
        } catch {
          // Handled via failed status report below.
        }
        report(button, false);
      }
    }

    document.addEventListener(
      "command",
      (event) => {
        if (typeof event.command === "string" && event.command.startsWith("--copy")) {
          copyTarget(event.source, event.target);
        }
      },
      true
    );

    if (!("CommandEvent" in window)) {
      document.addEventListener("click", (event) => {
        const button = event.target.closest('button[command^="--copy"]');
        if (!button) return;
        const target = document.getElementById(button.getAttribute("commandfor") || "");
        if (target) {
          copyTarget(button, target);
        }
      });
    }
  }

  /* 3. Accessible Interface Tabs & Callout Highlighter */
  function initTabs() {
    const tablist = document.querySelector("[data-tablist]");
    if (!tablist) return;

    const tabs = Array.from(tablist.querySelectorAll('[role="tab"]'));
    const panels = Array.from(document.querySelectorAll('[role="tabpanel"]'));

    function selectTab(targetTab, focusTab) {
      const panelId = targetTab.dataset.panelTarget;
      tabs.forEach((tab) => {
        const active = tab === targetTab;
        tab.setAttribute("aria-selected", String(active));
        tab.tabIndex = active ? 0 : -1;
      });
      panels.forEach((panel) => {
        const active = panel.id === panelId;
        if (active) {
          panel.removeAttribute("hidden");
        } else {
          panel.setAttribute("hidden", "until-found");
        }
      });
      if (focusTab) {
        targetTab.focus({ preventScroll: true });
      }
      targetTab.scrollIntoView({ block: "nearest", inline: "nearest" });
    }

    tablist.addEventListener("click", (event) => {
      const btn = event.target.closest('[role="tab"]');
      if (btn) selectTab(btn, false);
    });

    tablist.addEventListener("keydown", (event) => {
      const currentIndex = tabs.indexOf(document.activeElement);
      if (currentIndex === -1) return;

      let nextIndex = null;
      if (event.key === "ArrowRight") nextIndex = (currentIndex + 1) % tabs.length;
      else if (event.key === "ArrowLeft") nextIndex = (currentIndex - 1 + tabs.length) % tabs.length;
      else if (event.key === "Home") nextIndex = 0;
      else if (event.key === "End") nextIndex = tabs.length - 1;

      if (nextIndex !== null) {
        event.preventDefault();
        selectTab(tabs[nextIndex], true);
      }
    });

    panels.forEach((panel) => {
      panel.addEventListener("beforematch", () => {
        const matchingTab = tabs.find((t) => t.dataset.panelTarget === panel.id);
        if (matchingTab) selectTab(matchingTab, false);
      });

      const highlightBox = panel.querySelector("[data-highlight-box]");
      const callouts = Array.from(panel.querySelectorAll("[data-callout-btn]"));

      const activateCallout = (btn) => {
        callouts.forEach((item) => {
          item.setAttribute("aria-pressed", String(item === btn));
        });
        if (!highlightBox) return;
        const coords = (btn.dataset.box || "").split(",").map(Number);
        if (coords.length === 4 && coords.every(Number.isFinite)) {
          highlightBox.style.insetInlineStart = `${coords[0]}%`;
          highlightBox.style.insetBlockStart = `${coords[1]}%`;
          highlightBox.style.inlineSize = `${coords[2]}%`;
          highlightBox.style.blockSize = `${coords[3]}%`;
          highlightBox.classList.add("is-active");
        }
      };

      callouts.forEach((btn) => {
        btn.addEventListener("click", () => activateCallout(btn));
      });
    });
  }

  /* 4. Interactive Click-Through In-Game HUD Overlay Simulator & Code Copy */
  function initHudSimulator() {
    const stage = document.querySelector(".hud-overlay-preview");
    const peerButtons = Array.from(document.querySelectorAll("[data-hud-peer]"));
    const muteBtn = document.querySelector("[data-hud-mute]");
    const speakingOnlyBtn = document.querySelector("[data-hud-speaking-only]");
    const opacityInput = document.querySelector("[data-hud-opacity]");
    const opacityOutput = document.querySelector("[data-hud-opacity-val]");
    const emptyState = document.querySelector("[data-hud-empty]");
    const copyBtn = document.querySelector("[data-copy-code]");

    if (stage && peerButtons.length) {
      const state = {
        peers: {
          self: { speaking: true, muted: false },
          viper: { speaking: false, muted: false },
          lynx: { speaking: true, muted: false },
        },
        speakingOnly: false,
      };

      function syncHud() {
        let visibleCount = 0;

        for (const btn of peerButtons) {
          const id = btn.dataset.hudPeer;
          const peer = state.peers[id];
          const row = stage.querySelector(`[data-hud-row="${id}"]`);
          const pill = btn.querySelector("[data-peer-state]");
          if (!peer || !row) continue;

          const isSpeaking = peer.speaking && !peer.muted;
          row.classList.toggle("is-speaking", isSpeaking);
          row.classList.toggle("is-muted", peer.muted);

          const shouldHide = state.speakingOnly && !isSpeaking;
          row.hidden = shouldHide;
          if (!shouldHide) visibleCount += 1;

          btn.setAttribute("aria-pressed", String(isSpeaking));
          if (pill) {
            pill.textContent = peer.muted ? "Muted" : isSpeaking ? "Speaking" : "Quiet";
          }
        }

        if (muteBtn) {
          muteBtn.setAttribute("aria-pressed", String(state.peers.self.muted));
          const muteLabel = muteBtn.querySelector("[data-mute-label]");
          if (muteLabel) {
            muteLabel.textContent = state.peers.self.muted ? "Mic muted" : "Active";
          }
        }

        if (speakingOnlyBtn) {
          speakingOnlyBtn.setAttribute("aria-pressed", String(state.speakingOnly));
          const filterLabel = speakingOnlyBtn.querySelector("[data-filter-label]");
          if (filterLabel) {
            filterLabel.textContent = state.speakingOnly ? "On" : "Off";
          }
        }

        if (emptyState) {
          emptyState.hidden = visibleCount > 0;
        }
      }

      for (const btn of peerButtons) {
        btn.addEventListener("click", () => {
          const id = btn.dataset.hudPeer;
          const peer = state.peers[id];
          if (!peer) return;
          if (id === "self" && peer.muted) {
            peer.muted = false;
            peer.speaking = true;
          } else {
            peer.speaking = !peer.speaking;
          }
          syncHud();
        });
      }

      if (muteBtn) {
        muteBtn.addEventListener("click", () => {
          state.peers.self.muted = !state.peers.self.muted;
          syncHud();
        });
      }

      if (speakingOnlyBtn) {
        speakingOnlyBtn.addEventListener("click", () => {
          state.speakingOnly = !state.speakingOnly;
          syncHud();
        });
      }

      if (opacityInput) {
        opacityInput.addEventListener("input", () => {
          const pct = Number(opacityInput.value);
          stage.style.setProperty("--hud-opacity", (pct / 100).toFixed(2));
          if (opacityOutput) {
            opacityOutput.textContent = `${pct}%`;
          }
        });
      }

      syncHud();
    }

    if (copyBtn) {
      let resetTimer = 0;
      copyBtn.addEventListener("click", async () => {
        const code = copyBtn.dataset.copyCode || "482-910";
        try {
          if (navigator.clipboard && navigator.clipboard.writeText) {
            await navigator.clipboard.writeText(code);
          } else {
            const ta = document.createElement("textarea");
            ta.value = code;
            ta.setAttribute("readonly", "");
            ta.style.position = "fixed";
            ta.style.opacity = "0";
            document.body.appendChild(ta);
            ta.select();
            document.execCommand("copy");
            document.body.removeChild(ta);
          }
          copyBtn.textContent = "Copied";
          announce(`Copied demo voice code ${code} to clipboard`);
          window.clearTimeout(resetTimer);
          resetTimer = window.setTimeout(() => {
            copyBtn.textContent = "Copy code";
          }, 1800);
        } catch {
          copyBtn.textContent = "482-910";
          announce(`Copy failed. Demo code is ${code}`);
        }
      });
    }
  }

  /* 5. IntersectionObserver Reveals, Stagger & Benchmark Bar Counters */
  function animateCounter(el) {
    const target = Number(el.dataset.countTo);
    if (!Number.isFinite(target)) return;
    const prefix = el.dataset.countPrefix || "";
    const suffix = el.dataset.countSuffix || "";
    const decimals = Number(el.dataset.countDecimals || 0);

    if (prefersReducedMotion) {
      el.textContent = `${prefix}${target.toFixed(decimals)}${suffix}`;
      return;
    }

    const duration = 760;
    const start = performance.now();
    function tick(now) {
      const progress = Math.min(1, (now - start) / duration);
      const eased = 1 - Math.pow(1 - progress, 3);
      const val = (target * eased).toFixed(decimals);
      el.textContent = `${prefix}${val}${suffix}`;
      if (progress < 1) {
        window.requestAnimationFrame(tick);
      } else {
        el.textContent = `${prefix}${target.toFixed(decimals)}${suffix}`;
      }
    }
    window.requestAnimationFrame(tick);
  }

  function initReveals() {
    const revealTargets = document.querySelectorAll("[data-reveal]");
    const benchmarkGroups = document.querySelectorAll(".benchmark-group");

    // Assign clean sibling stagger inside card grids
    const staggerContainers = document.querySelectorAll(
      ".pillars-grid, .arch-grid, .crate-strip, .install-grid, .firstrun-grid, .telemetry-stack, .faq-list"
    );
    staggerContainers.forEach((container) => {
      const children = container.querySelectorAll(":scope > [data-reveal]");
      children.forEach((child, idx) => {
        child.style.setProperty("--reveal-delay", `${Math.min(idx * 80, 360)}ms`);
      });
    });

    if (prefersReducedMotion || !("IntersectionObserver" in window)) {
      revealTargets.forEach((el) => el.classList.add("is-visible"));
      benchmarkGroups.forEach((el) => {
        el.classList.add("is-visible");
        el.querySelectorAll("[data-count-to]").forEach(animateCounter);
      });
      return;
    }

    const revealObserver = new IntersectionObserver(
      (entries, obs) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add("is-visible");
            obs.unobserve(entry.target);
          }
        }
      },
      { threshold: 0.08, rootMargin: "0px 0px -72px 0px" }
    );

    const vh = window.innerHeight || 900;
    revealTargets.forEach((el) => {
      const rect = el.getBoundingClientRect();
      if (rect.top < vh * 0.85) {
        el.classList.add("is-visible");
      } else {
        revealObserver.observe(el);
      }
    });

    const benchObserver = new IntersectionObserver(
      (entries, obs) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add("is-visible");
            entry.target.querySelectorAll("[data-count-to]").forEach(animateCounter);
            obs.unobserve(entry.target);
          }
        }
      },
      { threshold: 0.25, rootMargin: "0px 0px -80px 0px" }
    );

    benchmarkGroups.forEach((group) => {
      const rect = group.getBoundingClientRect();
      if (rect.top < vh * 0.82) {
        group.classList.add("is-visible");
        group.querySelectorAll("[data-count-to]").forEach(animateCounter);
      } else {
        benchObserver.observe(group);
      }
    });
  }

  /* 6. B11 Tagline Word-by-Word Scroll Reveal (rAF-throttled only while in view) */
  function initTaglineReveal() {
    const heading = document.querySelector("[data-tagline-reveal]");
    if (!heading) return;

    const rawLines = (heading.getAttribute("data-tagline-lines") || heading.textContent || "").split("|");
    heading.textContent = "";

    const words = [];
    rawLines.forEach((line, lineIdx) => {
      const tokens = line.trim().split(/\s+/);
      tokens.forEach((token, idx) => {
        const span = document.createElement("span");
        span.className = "reveal-word";
        if (prefersReducedMotion) span.classList.add("is-lit");
        span.textContent = token;
        heading.appendChild(span);
        words.push(span);
        if (idx < tokens.length - 1) {
          heading.appendChild(document.createTextNode(" "));
        }
      });
      if (lineIdx < rawLines.length - 1) {
        heading.appendChild(document.createElement("br"));
      }
    });

    if (prefersReducedMotion || !words.length) return;

    let rafId = 0;
    let isVisible = false;

    function updateWords() {
      rafId = 0;
      const rect = heading.getBoundingClientRect();
      const vh = window.innerHeight || 800;
      const start = vh * 0.84;
      const end = vh * 0.34;
      const progress = Math.min(1, Math.max(0, (start - rect.top) / (start - end)));
      const litCount = Math.round(progress * words.length);

      for (let i = 0; i < words.length; i += 1) {
        words[i].classList.toggle("is-lit", i < litCount);
      }
    }

    function onScroll() {
      if (!isVisible || rafId) return;
      rafId = window.requestAnimationFrame(updateWords);
    }

    if ("IntersectionObserver" in window) {
      const io = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            isVisible = entry.isIntersecting;
            if (isVisible) {
              updateWords();
              window.addEventListener("scroll", onScroll, { passive: true });
            } else {
              window.removeEventListener("scroll", onScroll);
            }
          }
        },
        { rootMargin: "100px 0px" }
      );
      io.observe(heading);
    } else {
      for (const word of words) word.classList.add("is-lit");
    }
  }

  /* 7. Cross-Browser Smooth FAQ Accordion Animation */
  function initFaqAccordion() {
    const items = document.querySelectorAll(".faq-item");
    if (!items.length || prefersReducedMotion || typeof Element.prototype.animate !== "function") return;

    const easing = "cubic-bezier(0.22, 1, 0.36, 1)";

    items.forEach((details) => {
      const summary = details.querySelector(":scope > summary");
      const body = details.querySelector(":scope > .faq-body");
      if (!summary || !body) return;

      let activeAnim = null;

      summary.addEventListener("click", (event) => {
        event.preventDefault();
        if (activeAnim) {
          activeAnim.cancel();
          activeAnim = null;
        }

        if (details.open && !details.classList.contains("is-closing")) {
          const startHeight = body.offsetHeight;
          details.classList.add("is-closing");
          activeAnim = body.animate(
            [
              { height: `${startHeight}px`, paddingBottom: "24px", opacity: 1, transform: "translateY(0)" },
              { height: "0px", paddingBottom: "0px", opacity: 0, transform: "translateY(-4px)" },
            ],
            { duration: 280, easing }
          );
          activeAnim.onfinish = () => {
            details.open = false;
            details.classList.remove("is-closing");
            activeAnim = null;
          };
          activeAnim.oncancel = () => {
            details.classList.remove("is-closing");
          };
        } else {
          details.classList.remove("is-closing");
          details.open = true;
          const endHeight = body.offsetHeight;
          activeAnim = body.animate(
            [
              { height: "0px", paddingBottom: "0px", opacity: 0, transform: "translateY(-6px)" },
              { height: `${endHeight}px`, paddingBottom: "24px", opacity: 1, transform: "translateY(0)" },
            ],
            { duration: 320, easing }
          );
          activeAnim.onfinish = () => {
            activeAnim = null;
          };
        }
      });
    });
  }

  initNavbar();
  initSmoothAnchors();
  initPlatformDownload();
  initCopyButtons();
  initTabs();
  initHudSimulator();
  initReveals();
  initTaglineReveal();
  initFaqAccordion();
})();
