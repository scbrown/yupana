// Populate the sidebar
//
// This is a script, and not included directly in the page, to control the total size of the book.
// The TOC contains an entry for each page, so if each page includes a copy of the TOC,
// the total size of the page becomes O(n**2).
class MDBookSidebarScrollbox extends HTMLElement {
    constructor() {
        super();
    }
    connectedCallback() {
        this.innerHTML = '<ol class="chapter"><li class="chapter-item expanded affix "><a href="index.html">Introduction</a></li><li class="chapter-item expanded affix "><li class="part-title">Getting Started</li><li class="chapter-item expanded "><a href="getting-started/installation.html"><strong aria-hidden="true">1.</strong> Installation</a></li><li class="chapter-item expanded "><a href="getting-started/quick-start.html"><strong aria-hidden="true">2.</strong> Quick Start</a></li><li class="chapter-item expanded "><a href="getting-started/configuration.html"><strong aria-hidden="true">3.</strong> Configuration</a></li><li class="chapter-item expanded "><a href="getting-started/harness-integration.html"><strong aria-hidden="true">4.</strong> Harness Integration</a></li><li class="chapter-item expanded affix "><li class="part-title">Concepts</li><li class="chapter-item expanded "><a href="concepts/architecture.html"><strong aria-hidden="true">5.</strong> Architecture</a></li><li class="chapter-item expanded "><a href="concepts/tenancy-model.html"><strong aria-hidden="true">6.</strong> The Tenancy Model</a></li><li class="chapter-item expanded "><a href="concepts/tiers-and-freshness.html"><strong aria-hidden="true">7.</strong> Tiers and Freshness</a></li><li class="chapter-item expanded "><a href="concepts/promotion.html"><strong aria-hidden="true">8.</strong> Promotion to Quipu</a></li><li class="chapter-item expanded "><a href="concepts/game-state.html"><strong aria-hidden="true">9.</strong> The Game-State Harness</a></li><li class="chapter-item expanded affix "><li class="part-title">Reference</li><li class="chapter-item expanded "><a href="reference/cli.html"><strong aria-hidden="true">10.</strong> CLI Commands</a></li><li class="chapter-item expanded "><a href="reference/config.html"><strong aria-hidden="true">11.</strong> Configuration Reference</a></li><li class="chapter-item expanded "><a href="reference/mcp-tools.html"><strong aria-hidden="true">12.</strong> MCP Tools</a></li><li class="chapter-item expanded "><a href="reference/daemon.html"><strong aria-hidden="true">13.</strong> Resident Daemon</a></li><li class="chapter-item expanded "><a href="reference/policy-guard.html"><strong aria-hidden="true">14.</strong> Pre-Edit Policy Guard</a></li><li class="chapter-item expanded "><a href="reference/enforcement-trace.html"><strong aria-hidden="true">15.</strong> The Enforcement Trace</a></li><li class="chapter-item expanded affix "><li class="part-title">Design</li><li class="chapter-item expanded "><a href="design/specification.html"><strong aria-hidden="true">16.</strong> Specification</a></li><li class="chapter-item expanded "><a href="design/vision.html"><strong aria-hidden="true">17.</strong> Vision</a></li><li class="chapter-item expanded "><a href="design/policy-edit-hooks.html"><strong aria-hidden="true">18.</strong> Policy edit hooks</a></li><li class="chapter-item expanded "><a href="design/governance-plane.html"><strong aria-hidden="true">19.</strong> Governance Plane</a></li><li class="chapter-item expanded "><a href="design/sarc-conformance.html"><strong aria-hidden="true">20.</strong> SARC Conformance</a></li><li class="chapter-item expanded "><a href="design/governed-relations.html"><strong aria-hidden="true">21.</strong> Governed Relations</a></li><li class="chapter-item expanded "><a href="design/workflow-gated-edits.html"><strong aria-hidden="true">22.</strong> Workflow-Gated Edits</a></li><li class="chapter-item expanded affix "><li class="part-title">Operations</li><li class="chapter-item expanded "><a href="reference/contributing.html"><strong aria-hidden="true">23.</strong> Contributing</a></li></ol>';
        // Set the current, active page, and reveal it if it's hidden
        let current_page = document.location.href.toString().split("#")[0].split("?")[0];
        if (current_page.endsWith("/")) {
            current_page += "index.html";
        }
        var links = Array.prototype.slice.call(this.querySelectorAll("a"));
        var l = links.length;
        for (var i = 0; i < l; ++i) {
            var link = links[i];
            var href = link.getAttribute("href");
            if (href && !href.startsWith("#") && !/^(?:[a-z+]+:)?\/\//.test(href)) {
                link.href = path_to_root + href;
            }
            // The "index" page is supposed to alias the first chapter in the book.
            if (link.href === current_page || (i === 0 && path_to_root === "" && current_page.endsWith("/index.html"))) {
                link.classList.add("active");
                var parent = link.parentElement;
                if (parent && parent.classList.contains("chapter-item")) {
                    parent.classList.add("expanded");
                }
                while (parent) {
                    if (parent.tagName === "LI" && parent.previousElementSibling) {
                        if (parent.previousElementSibling.classList.contains("chapter-item")) {
                            parent.previousElementSibling.classList.add("expanded");
                        }
                    }
                    parent = parent.parentElement;
                }
            }
        }
        // Track and set sidebar scroll position
        this.addEventListener('click', function(e) {
            if (e.target.tagName === 'A') {
                sessionStorage.setItem('sidebar-scroll', this.scrollTop);
            }
        }, { passive: true });
        var sidebarScrollTop = sessionStorage.getItem('sidebar-scroll');
        sessionStorage.removeItem('sidebar-scroll');
        if (sidebarScrollTop) {
            // preserve sidebar scroll position when navigating via links within sidebar
            this.scrollTop = sidebarScrollTop;
        } else {
            // scroll sidebar to current active section when navigating via "next/previous chapter" buttons
            var activeSection = document.querySelector('#sidebar .active');
            if (activeSection) {
                activeSection.scrollIntoView({ block: 'center' });
            }
        }
        // Toggle buttons
        var sidebarAnchorToggles = document.querySelectorAll('#sidebar a.toggle');
        function toggleSection(ev) {
            ev.currentTarget.parentElement.classList.toggle('expanded');
        }
        Array.from(sidebarAnchorToggles).forEach(function (el) {
            el.addEventListener('click', toggleSection);
        });
    }
}
window.customElements.define("mdbook-sidebar-scrollbox", MDBookSidebarScrollbox);
