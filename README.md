# JHP

_JHP_ is an experimental web backend JavaScript runtime that executes combined HTML and JavaScript code, enabling developers to write dynamic web content by processing both markup and scripts in a single execution flow.

## Example

```php
<?
let msg = "This is a message";
echo('Another message here');
?>

<html>
<body>
  <?= msg ?>
</body>
</html>
```

## Benchmark results

```console
$ wrk -H 'Accept: application/json,text/html;q=0.9,application/xhtml+xml;q=0.9,application/xml;q=0.8,*/*;q=0.7' \
          -H 'Connection: keep-alive' \
          --latency -d 15 -c 256 --timeout 8 -t 8 \
          http://localhost:8090/
```

```console
Running 15s test @ http://localhost:8090/
  8 threads and 256 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency    21.11ms   15.49ms 131.27ms   76.64%
    Req/Sec     1.65k   285.14     6.18k    77.51%
  Latency Distribution
     50%   17.44ms
     75%   28.18ms
     90%   41.12ms
     99%   74.59ms
  197889 requests in 15.09s, 73.13MB read
Requests/sec:  13109.91
Transfer/sec:      4.84MB
```

