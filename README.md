# scigantic-cryoem-context

Reference context for cryo-EM ingestion on a Kafka bus.

A facility's ingestion pipeline publishes one event per file it sees. This
service reads those events, looks up the EMDB and EMPIAR entries the
collection is meant to resemble, attaches what they say about the instrument
setup, compares the file's header against it, and republishes the event with
that context and a verdict. A file collected at the wrong magnification or
voltage is marked `fail` before it reaches processing.

Rust library plus one binary, `cryoem-context-bridge`. Kafka through
librdkafka (vendored, with TLS). Reference data from the public EBI REST APIs,
cached in memory.

## Run

    cargo build --release
    ./target/release/cryoem-context-bridge run \
        --brokers broker1:9092,broker2:9092 \
        --in cryoem.ingest --out cryoem.ingest.enriched --dlq cryoem.ingest.dlq

Every flag is also an environment variable (`CRYOEM_BRIDGE_BROKERS`,
`CRYOEM_BRIDGE_IN`, ...). Extra librdkafka properties pass through `--kafka`:

    --kafka security.protocol=SASL_SSL --kafka sasl.mechanism=SCRAM-SHA-512 \
    --kafka sasl.username=... --kafka sasl.password=...

Without a broker:

    # what EMPIAR-10973 and its paired EMDB entry say the setup was
    cryoem-context-bridge lookup --empiar 10973 --brief

    # check one event file; exit 1 on fail, 2 on unknown
    cryoem-context-bridge check examples/event.eer.json
    cryoem-context-bridge check examples/event.mdoc-wrong-voltage.json

`examples/docker-compose.yml` starts a local Redpanda for an end-to-end try.

## Deploying on k3s

`deploy/Dockerfile` builds the binary into a distroless image (no shell, runs
as nonroot). CI pushes it to `ghcr.io/scigantic/scigantic-cryoem-context` on
every push to main (`latest`, `sha-...`) and on tags (`0.1.0`), for amd64 and
arm64.

`deploy/k3s/` is a kustomization with a one-broker Kafka in KRaft mode on a
PersistentVolumeClaim, a Job that creates the three topics, and the bridge
Deployment with readiness and liveness probes on `/healthz`:

    kubectl apply -k deploy/k3s
    kubectl -n cryoem wait --for=condition=available deploy/cryoem-context-bridge --timeout=300s

    # produce an event from inside the cluster and read the enriched one back
    kubectl -n cryoem exec kafka-0 -- sh -c \
      '/opt/kafka/bin/kafka-console-producer.sh --bootstrap-server localhost:9092 --topic cryoem.ingest' \
      < examples/event.eer.json
    kubectl -n cryoem exec kafka-0 -- /opt/kafka/bin/kafka-console-consumer.sh \
      --bootstrap-server localhost:9092 --topic cryoem.ingest.enriched --from-beginning --max-messages 1

The bridge's settings are the ConfigMap in `deploy/k3s/bridge.yaml`; set
`CRYOEM_BRIDGE_EMPIAR` or `CRYOEM_BRIDGE_EMDB` there to judge every event
against one collection. The health endpoint (`--health-addr`, here
`0.0.0.0:8080`) answers any GET with 200 and the counters as JSON. SIGTERM
stops the loop after the in-flight message, so a rolling restart does not
drop or duplicate events. To use a broker that already exists, drop
`kafka.yaml` and `topics.yaml` from the kustomization and point
`CRYOEM_BRIDGE_BROKERS` at it; TLS and SASL go through `--kafka` args.

The single-broker Kafka is for a box that has none. It has one disk and no
replication; a facility that already runs Strimzi or Redpanda should keep
that and deploy only `bridge.yaml`.

## Events

Input is the per-file record `scigantic-edge` writes, with an optional
`reference` naming the archive entries to compare against:

    {
      "reference": {"empiar": "10973", "emdb": "EMD-14173"},
      "source": "/mnt/glacios/session1/FoilHole_0001.eer",
      "sizeBytes": 147199534,
      "mtime": 1761700000,
      "decoded": {"format": "eer", "fields": {"pixelSizeA": 0.43, "commercialName": "Falcon 4i", ...}}
    }

The reference can instead come from the Kafka headers `scigantic-empiar` and
`scigantic-emdb`, or from `--empiar` / `--emdb` on the command line, which
apply to every event that names none. Body beats headers beats defaults. An
EMPIAR id alone is enough: the entry's EMDB cross-reference is followed.
Producers that are not scigantic-edge can send
`{"setup": {"pixelSizeA": 0.43, "voltageKv": 200, ...}}` using the canonical
keys below.

Output, on `--out`, keyed by the input key (or `source`):

    {
      "source": "...",
      "event": { the input, unchanged },
      "observed": {"format": "eer", "pixelSizeA": 0.43, "detector": "Falcon 4i", "imageWidth": 4096, ...},
      "reference": {
        "key": {"empiar": "EMPIAR-10973", "emdb": "EMD-14173"},
        "title": "1.6Å Apoferritin from 200kV Glacios with Selectris Falcon 4",
        "expected": {"pixelSizeA": 0.43, "voltageKv": 200, "detector": "FEI FALCON IV (4k x 4k)",
                     "dataFormat": "EER", "framesPerImage": 371, "dosePerImageEA2": 40,
                     "defocusMinUm": 0.5, "defocusMaxUm": 0.9, "microscope": "TFS GLACIOS",
                     "resolutionA": 1.635, "provenance": {"pixelSizeA": "EMPIAR-10973", "voltageKv": "EMD-14173", ...}},
        "empiar": { the flattened EMPIAR entry },
        "emdb": { the flattened EMDB entry }
      },
      "qc": {
        "status": "pass",
        "findings": [],
        "checked": ["pixelSizeA", "detector", "dataFormat"],
        "unobserved": ["voltageKv", "dosePerImageEA2"],
        "unreferenced": ["imageWidth", "imageHeight"]
      },
      "bridge": {"version": "0.1.0", "processedAtUnix": 1761700100, "referenceKey": "EMPIAR-10973|EMD-14173"}
    }

Headers on the output message: `scigantic-qc-verdict` (`pass`, `warn`,
`fail`, `unknown`) and `scigantic-reference` (`EMPIAR-10973,EMD-14173`), so a
consumer can route on the verdict without parsing the body.

Payloads that are not JSON go to `--dlq` with a `scigantic-bridge-error`
header, or are logged and skipped when no DLQ is set. An event whose
reference cannot be resolved is passed through with status `unknown` and the
reason in `findings`; it is not dropped.

Offsets are stored after the enriched message is acknowledged. A crash
replays, it does not lose.

## Checks

Observed values come from the header fields `scigantic-edge` emits for `mrc`,
`eer`, `epu-image` and `serialem-mdoc`, mapped onto the canonical keys of its
setup check. Expected values come from EMPIAR for image-set facts and EMDB for
microscope facts.

| key | observed from | expected from | tolerance | on mismatch |
|---|---|---|---|---|
| `pixelSizeA` | header pixel size | EMPIAR image set `pixel_width` | 2% | fail |
| `voltageKv` | EPU, mdoc, FEI MRC | EMDB `acceleration_voltage` | 1% | fail |
| `imageWidth`, `imageHeight` | header dimensions | EMPIAR image set, else EMDB digitization | exact | warn |
| `detector` | EPU camera name, EER commercial name | EMDB `film_or_detector_model` | detector family | warn |
| `dosePerImageEA2` | mdoc `ExposureDose`, EPU dose | EMDB `average_electron_dose_per_image` | 25% | warn |
| `dataFormat` | header format | EMPIAR image set `data_format` | | info |

A key present on only one side is listed in `unobserved` or `unreferenced`
and not judged. If nothing is comparable the status is `unknown`. The EMDB
map's `pixel_spacing` is reported but never used as the expected pixel size:
it is the deposited map's spacing after binning, not the acquisition's
(EMD-14173 is 0.605 A; its movies in EMPIAR-10973 are 0.43 A).

Tolerances are flags: `--tol-pixel`, `--tol-voltage`, `--tol-dose`.

## Library

    use scigantic_cryoem_context::{Resolver, ReferenceKey, Observed, qc, Tolerances};

    let resolver = Resolver::with_defaults();
    let ctx = resolver.resolve(&ReferenceKey { empiar: Some("10973".into()), emdb: None }).await?;
    let obs = Observed::from_decoded("eer", &fields);
    let verdict = qc::check(&obs, &ctx.expected, &Tolerances::default());

`emdb::EmdbClient` and `empiar::EmpiarClient` are usable on their own and
accept a base URL for a mirror or a test server. `bus::process` is the
per-message step without Kafka; `bus::Bridge` is the loop.

## Layout

    src/emdb.rs      EMDB REST client and flattening
    src/empiar.rs    EMPIAR REST client and flattening
    src/context.rs   ReferenceKey, Expected, ReferenceContext::merge
    src/observed.rs  canonical setup values from a decoded header
    src/qc.rs        comparison and verdict
    src/resolve.rs   cached lookup
    src/message.rs   input and output JSON shapes, header names
    src/bus.rs       Kafka consume, enrich, produce
    src/health.rs    probe endpoint
    src/bin/bridge.rs
    deploy/          Dockerfile and the k3s kustomization
    tests/           fixtures are real API responses for EMD-11638, EMD-14173, EMPIAR-10973;
                     tests/bus_mock.rs runs the loop on librdkafka's in-process mock cluster

    cargo test

## Not done

- Only `mrc`, `eer`, `epu-image` and `serialem-mdoc` headers are mapped.
  Other formats are passed through with whatever canonical keys the producer
  already flattened.
- No lookup by sample or organism. The event has to say which entries it
  resembles.
- EBI has no bulk endpoint here; one HTTP call per archive per distinct
  reference, then cached for `--cache-ttl` seconds (default 3600).
- Compression is lz4. zstd is not compiled into the vendored librdkafka.
