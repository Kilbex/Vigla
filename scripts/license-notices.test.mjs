import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  generateLicenseNotices,
  legalDistributionFiles,
} from "./generate-license-notices.mjs";
import {
  retainedArchiveAttributions,
  retainedMetadataAttributions,
  retainedRemoteLicenses,
  verifyRetainedRemoteLicenses,
} from "./retained-license-sources.mjs";

const fonts = [
  {
    packageName: "@fontsource-variable/inter",
    version: "5.2.8",
    licenseId: "OFL-1.1",
    licenseFile: "inter-OFL-1.1.txt",
    copyright:
      "Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter) Inter-Italic[opsz,wght].ttf: Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter)",
  },
  {
    packageName: "@fontsource-variable/jetbrains-mono",
    version: "5.2.8",
    licenseId: "OFL-1.1",
    licenseFile: "jetbrains-mono-OFL-1.1.txt",
    copyright:
      "Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono) JetBrainsMono-Italic[wght].ttf: Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono)",
  },
];

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

test("the published notice covers each locked bundled font", async () => {
  const [notice, publicNotice, lockfile] = await Promise.all([
    readFile(new URL("../THIRD_PARTY_NOTICES.md", import.meta.url), "utf8"),
    readFile(
      new URL("../app/public/THIRD_PARTY_NOTICES.md", import.meta.url),
      "utf8",
    ),
    readFile(new URL("../pnpm-lock.yaml", import.meta.url), "utf8"),
  ]);

  assert.equal(publicNotice, notice);

  for (const font of fonts) {
    const packageManifest = JSON.parse(
      await readFile(
        new URL(
          `../app/node_modules/${font.packageName}/package.json`,
          import.meta.url,
        ),
        "utf8",
      ),
    );

    assert.equal(packageManifest.version, font.version);
    assert.equal(packageManifest.license, font.licenseId);
    assert.match(
      lockfile,
      new RegExp(
        `^  '${escapeRegExp(font.packageName)}@${escapeRegExp(font.version)}':$`,
        "m",
      ),
    );
    const heading = `## ${font.packageName} ${font.version}`;
    const sectionStart = notice.indexOf(heading);
    assert.notEqual(sectionStart, -1);

    const nextSection = notice.indexOf("\n## ", sectionStart + heading.length);
    const section = notice.slice(
      sectionStart,
      nextSection === -1 ? undefined : nextSection,
    );

    assert.ok(section.includes(font.licenseId));
    assert.ok(section.includes(font.copyright));
    assert.ok(section.includes(`third_party_licenses/${font.licenseFile}`));
  }
});

test("checked-in and web-bundled font licenses match their installed sources", async () => {
  for (const font of fonts) {
    const [installedLicense, sourceLicense, publicLicense] = await Promise.all([
      readFile(
        new URL(
          `../app/node_modules/${font.packageName}/LICENSE`,
          import.meta.url,
        ),
        "utf8",
      ),
      readFile(
        new URL(`../third_party_licenses/${font.licenseFile}`, import.meta.url),
        "utf8",
      ),
      readFile(
        new URL(
          `../app/public/third_party_licenses/${font.licenseFile}`,
          import.meta.url,
        ),
        "utf8",
      ),
    ]);

    assert.equal(sourceLicense, installedLicense);
    assert.equal(publicLicense, sourceLicense);
    assert.ok(sourceLicense.startsWith(`${font.copyright}\n\n`));
    assert.match(sourceLicense, /SIL OPEN FONT LICENSE Version 1\.1/);
  }
});

test("the adapted date algorithm retains its MIT attribution in distributions", async () => {
  const [notice, sourceLicense, publicLicense, source] = await Promise.all([
    readFile(new URL("../THIRD_PARTY_NOTICES.md", import.meta.url), "utf8"),
    readFile(
      new URL(
        "../third_party_licenses/howard-hinnant-date-MIT.txt",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "../app/public/third_party_licenses/howard-hinnant-date-MIT.txt",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL("../crates/event-schema/src/time.rs", import.meta.url),
      "utf8",
    ),
  ]);

  assert.equal(publicLicense, sourceLicense);
  assert.match(notice, /## Howard Hinnant date algorithms/);
  assert.match(notice, /License: `MIT`/);
  assert.match(notice, /Copyright \(c\) 2015, 2016, 2017 Howard Hinnant/);
  assert.match(sourceLicense, /The MIT License \(MIT\)/);
  assert.match(sourceLicense, /Copyright \(c\) 2019 Jiangang Zhuang/);
  assert.match(source, /third_party_licenses\/howard-hinnant-date-MIT\.txt/);
});

test("the project notice is bundled unchanged", async () => {
  const [sourceNotice, publicNotice] = await Promise.all([
    readFile(new URL("../NOTICE", import.meta.url), "utf8"),
    readFile(new URL("../app/public/NOTICE", import.meta.url), "utf8"),
  ]);

  assert.equal(publicNotice, sourceNotice);
  assert.match(sourceNotice, /Copyright 2026 Kilbex and Vigla contributors/);
  assert.match(sourceNotice, /Apache License, Version 2\.0/);
  assert.match(sourceNotice, /THIRD_PARTY_NOTICES\.md/);
});

test("the production dependency aggregate is path-independent and web-bundled", async () => {
  const [aggregate, publicAggregate, license, publicLicense] =
    await Promise.all([
      readFile(new URL("../THIRD_PARTY_NOTICES.txt", import.meta.url), "utf8"),
      readFile(
        new URL("../app/public/THIRD_PARTY_NOTICES.txt", import.meta.url),
        "utf8",
      ),
      readFile(new URL("../LICENSE", import.meta.url), "utf8"),
      readFile(new URL("../app/public/LICENSE", import.meta.url), "utf8"),
    ]);

  assert.equal(publicAggregate, aggregate);
  assert.equal(publicLicense, license);
  assert.equal(aggregate.includes(process.cwd()), false);
  assert.doesNotMatch(aggregate, /\/Users\//);
  assert.match(aggregate, /@xterm\/xterm 6\.0\.0/);
  assert.match(aggregate, /@xyflow\/react 12\.10\.2/);
  assert.match(aggregate, /d3-selection 3\.0\.0/);
  assert.match(aggregate, /Declared license: BSD-3-Clause/);
  assert.match(aggregate, /Declared license: ISC/);
  assert.match(aggregate, /Declared license: MIT/);
  assert.match(aggregate, /LOCKED PRODUCTION RUST DEPENDENCY INVENTORY/);
  assert.match(
    aggregate,
    /icu_normalizer 2\.2\.0 — Declared license: Unicode-3\.0/,
  );
  assert.match(aggregate, /UNICODE LICENSE V3/);
  assert.match(
    aggregate,
    /STATIC NATIVE COMPONENTS ENABLED BY SHIPPING FEATURES/,
  );
  assert.match(
    aggregate,
    /ONNX Runtime 1\.24\.2 — Linked by ort-sys@2\.0\.0-rc\.12/,
  );
});

test("the optional embeddings build retains exact ONNX Runtime legal files", async () => {
  const [reportText, cargoLock, buildScript, hostManifest, curatedNotice] =
    await Promise.all([
      readFile(
        new URL(
          "../third_party_licenses/rust-dependencies.json",
          import.meta.url,
        ),
        "utf8",
      ),
      readFile(new URL("../Cargo.lock", import.meta.url), "utf8"),
      readFile(new URL("./build.sh", import.meta.url), "utf8"),
      readFile(new URL("../app/src-tauri/Cargo.toml", import.meta.url), "utf8"),
      readFile(new URL("../THIRD_PARTY_NOTICES.md", import.meta.url), "utf8"),
      verifyRetainedRemoteLicenses(),
    ]);
  const report = JSON.parse(reportText);
  const component = report.native_components.find(
    (candidate) => candidate.name === "ONNX Runtime",
  );
  const onnxSources = retainedRemoteLicenses.filter(
    (source) => source.component === "ONNX Runtime",
  );

  assert.deepEqual(component, {
    name: "ONNX Runtime",
    version: "1.24.2",
    linked_by: "ort-sys@2.0.0-rc.12",
    legal_files: onnxSources.map((source) => ({
      file_name: source.fileName,
      source_url: source.url,
      sha256: source.sha256,
    })),
  });
  assert.match(cargoLock, /name = "fastembed"\nversion = "5\.13\.4"/);
  assert.match(cargoLock, /name = "ort"\nversion = "2\.0\.0-rc\.12"/);
  assert.match(cargoLock, /name = "ort-sys"\nversion = "2\.0\.0-rc\.12"/);
  assert.match(buildScript, /TAURI_BUILD_ARGS\+=\(--features embeddings\)/);
  assert.match(hostManifest, /embeddings = \["orchestrator\/embeddings"\]/);
  assert.match(curatedNotice, /## ONNX Runtime 1\.24\.2/);

  for (const source of retainedRemoteLicenses) {
    const [retained, publicCopy] = await Promise.all([
      readFile(new URL(`../third_party_licenses/${source.fileName}`, import.meta.url)),
      readFile(
        new URL(
          `../app/public/third_party_licenses/${source.fileName}`,
          import.meta.url,
        ),
      ),
    ]);
    assert.deepEqual(publicCopy, retained);
  }
});

test("the aggregate covers every locked production package and exact retained text", async () => {
  const [checkedIn, generated] = await Promise.all([
    readFile(new URL("../THIRD_PARTY_NOTICES.txt", import.meta.url), "utf8"),
    generateLicenseNotices(),
  ]);

  assert.equal(checkedIn, generated.text);
  for (const packageRecord of generated.packages) {
    assert.ok(
      generated.text.includes(
        `- ${packageRecord.name} ${packageRecord.version} — ` +
          `Declared license: ${packageRecord.license}`,
      ),
    );
    for (const licenseFile of packageRecord.files) {
      assert.ok(generated.text.includes(licenseFile.text.trimEnd()));
    }
  }
  for (const packageRecord of generated.rustPackages) {
    assert.ok(
      generated.text.includes(
        `- ${packageRecord.name} ${packageRecord.version} — ` +
          `Declared license: ${packageRecord.declared_license}; ` +
          `Selected: ${packageRecord.selected_licenses.join(", ")}; ` +
          `Source: ${packageRecord.source_url}`,
      ),
    );
  }
  for (const license of generated.rustLicenses) {
    assert.ok(generated.text.includes(license.text.trimEnd()));
  }
  for (const notice of generated.rustNotices) {
    const sourceLabel = notice.provenance
      ? `(${notice.provenance}: ${notice.source_file}; ` +
        (notice.source_url ? "source: " : "SHA-256: ")
      : `(retained bundled legal file: ${notice.source_file})`;
    assert.ok(generated.text.includes(sourceLabel));
    assert.ok(generated.text.includes(notice.text.trimEnd()));
  }
  for (const component of generated.nativeComponents) {
    assert.ok(
      generated.text.includes(
        `- ${component.name} ${component.version} — ` +
          `Linked by ${component.linked_by}`,
      ),
    );
    for (const legalFile of component.legal_files) {
      assert.ok(
        generated.text.includes(`third_party_licenses/${legalFile.file_name}`),
      );
    }
  }
  for (const attribution of generated.rustMetadataAttributions) {
    assert.ok(generated.text.includes(`- ${attribution.package} —`));
    assert.ok(generated.text.includes(attribution.limitation));
  }
});

test("the retained Rust report is policy- and lock-bound without local paths", async () => {
  const [reportText, cargoLock, aboutConfig] = await Promise.all([
    readFile(
      new URL(
        "../third_party_licenses/rust-dependencies.json",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(new URL("../Cargo.lock", import.meta.url), "utf8"),
    readFile(new URL("../about.toml", import.meta.url), "utf8"),
  ]);
  const report = JSON.parse(reportText);
  const { createHash } = await import("node:crypto");

  assert.equal(
    report.cargo_lock_sha256,
    createHash("sha256").update(cargoLock).digest("hex"),
  );
  assert.equal(
    report.about_config_sha256,
    createHash("sha256").update(aboutConfig).digest("hex"),
  );
  assert.equal(report.generator, "cargo-about@0.9.1");
  assert.ok(report.packages.length > 400);
  assert.ok(
    report.packages.every((packageRecord) =>
      packageRecord.source_url.startsWith("https://crates.io/crates/"),
    ),
  );
  assert.ok(report.licenses.some((license) => license.id === "Unicode-3.0"));
  const attributionCoverage = new Set(
    [...report.notices, ...report.metadata_attributions].flatMap((record) =>
      record.attribution_license_ids.map(
        (licenseId) => `${record.package}\0${licenseId}`,
      ),
    ),
  );
  for (const license of report.licenses) {
    assert.doesNotMatch(license.text, /<(?:year|owner|copyright holders)>/i);
    if (license.template_placeholders_removed) {
      for (const packageName of license.packages) {
        assert.ok(
          attributionCoverage.has(`${packageName}\0${license.id}`),
          `${packageName} lacks an exact ${license.id} attribution source`,
        );
      }
    }
  }
  assert.deepEqual(
    report.metadata_attributions,
    retainedMetadataAttributions.map((attribution) => ({
      package: attribution.package,
      attribution_license_ids: [...attribution.licenseIds],
      declared_license: attribution.declaredLicense,
      authors: [...attribution.authors],
      repository: attribution.repository,
      vcs_commit: attribution.vcsCommit,
      limitation: attribution.limitation,
    })),
  );
  for (const source of retainedRemoteLicenses.filter((item) => item.packages)) {
    for (const packageName of source.packages) {
      assert.ok(
        report.notices.some(
          (notice) =>
            notice.package === packageName &&
            notice.source_file ===
              `third_party_licenses/${source.fileName}` &&
            notice.source_url === source.url &&
            notice.sha256 === source.sha256 &&
            JSON.stringify(notice.attribution_license_ids) ===
              JSON.stringify(source.licenseIds ?? []),
        ),
        `${packageName} lacks its checksum-pinned upstream legal file`,
      );
    }
  }
  for (const source of retainedArchiveAttributions) {
    assert.ok(
      report.notices.some(
        (notice) =>
          notice.package === source.package &&
          notice.source_file === source.sourceFile &&
          notice.sha256 === source.sha256 &&
          notice.provenance === "checksum-pinned crate archive legal file" &&
          JSON.stringify(notice.attribution_license_ids) ===
            JSON.stringify(source.licenseIds),
      ),
      `${source.package} lacks its checksum-pinned crate-archive attribution`,
    );
  }
  const brotli = report.notices.filter(
    (notice) => notice.package === "brotli@8.0.2",
  );
  assert.deepEqual(
    brotli.find((notice) => notice.source_file === "LICENSE.MIT")
      ?.attribution_license_ids,
    [],
  );
  assert.deepEqual(
    brotli.find(
      (notice) =>
        notice.source_file ===
        "third_party_licenses/brotli-8.0.2-LICENSE.BSD-3-Clause.txt",
    )?.attribution_license_ids,
    ["BSD-3-Clause"],
  );
  assert.deepEqual(
    report.notices.find(
      (notice) =>
        notice.package === "objc2@0.6.4" &&
        notice.source_file ===
          "third_party_licenses/objc2-core-0.6.4-LICENSE.md",
    )?.attribution_license_ids,
    [],
  );
  assert.ok(
    report.notices.some(
      (notice) =>
        notice.package.startsWith("onig_sys@") &&
        notice.source_file === "oniguruma/COPYING",
      ),
  );
  for (const [packagePrefix, sourceFile] of [
    ["brotli-decompressor@", "LICENSE"],
    ["cargo_toml@", "LICENSE"],
    ["dpi@", "LICENSE-LIBM-MIT"],
    ["exr@", "LICENSE.md"],
    ["siphasher@", "COPYING"],
    ["tauri@", "LICENSE_MIT"],
  ]) {
    assert.ok(
      report.notices.some(
        (notice) =>
          notice.package.startsWith(packagePrefix) &&
          notice.source_file === sourceFile,
      ),
      `${packagePrefix} lacks ${sourceFile}`,
    );
  }
  assert.match(reportText, /Copyright 2017 Josh Teeter/);
  assert.match(reportText, /Copyright \(c\) 2016 Dropbox, Inc\./);
  assert.match(reportText, /Copyright \(c\) 2022 Oscar Beaumont/);
  assert.ok(
    report.notices.some(
      (notice) =>
        notice.package.startsWith("rav1e@") &&
        notice.source_file === "PATENTS",
    ),
  );
  assert.ok(
    report.notices.some(
      (notice) =>
        notice.package.startsWith("parking@") &&
        notice.source_file === "LICENSE-THIRD-PARTY",
    ),
  );
  assert.ok(
    report.notices.some(
      (notice) =>
        notice.package.startsWith("security-framework@") &&
        notice.source_file === "THIRD_PARTY",
    ),
  );
  assert.match(reportText, /Alliance for Open Media Patent License 1\.0/);
  assert.ok(
    report.notices.some(
      (notice) =>
        notice.package.startsWith("zstd-sys@") &&
        notice.source_file === "zstd/COPYING",
    ),
  );
  assert.doesNotMatch(reportText, /\/Users\//);
  assert.equal(reportText.includes(process.cwd()), false);
});

test("runtime-imported layout code is a production dependency", async () => {
  const packageManifest = JSON.parse(
    await readFile(new URL("../app/package.json", import.meta.url), "utf8"),
  );

  assert.equal(packageManifest.dependencies["@dagrejs/dagre"], "^3.0.0");
  assert.equal(packageManifest.devDependencies["@dagrejs/dagre"], undefined);
});

test("Pages and local Tauri bundles retain the canonical legal files", async () => {
  const [siteBuilder, tauriConfig] = await Promise.all([
    readFile(new URL("./build-site.mjs", import.meta.url), "utf8"),
    readFile(
      new URL("../app/src-tauri/tauri.conf.json", import.meta.url),
      "utf8",
    ).then(JSON.parse),
  ]);

  assert.match(siteBuilder, /copyLegalDistribution\(siteDist\)/);
  const resources = tauriConfig.bundle.resources;
  for (const name of legalDistributionFiles) {
    assert.equal(resources[`../../${name}`], `licenses/${name}`);
  }
  assert.equal(
    resources["../../third_party_licenses/"],
    "licenses/third_party_licenses/",
  );
  const absolutePathPattern = /^(?:\/|[A-Za-z]:[\\/])/;
  for (const [source, destination] of Object.entries(resources)) {
    assert.doesNotMatch(source, absolutePathPattern);
    assert.doesNotMatch(destination, absolutePathPattern);
  }
});
