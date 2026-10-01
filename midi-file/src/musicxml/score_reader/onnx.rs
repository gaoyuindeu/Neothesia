//! Just enough of the ONNX format (protobuf) to run the detector on the GPU: the graph's
//! nodes in order, their attributes and the weights.

use std::collections::HashMap;

pub struct Tensor {
    pub dims: Vec<usize>,
    pub data: Vec<f32>,
}

pub enum Attr {
    Int(i64),
    Ints(Vec<i64>),
    Str(String),
    Tensor(Tensor),
    Other,
}

pub struct Node {
    pub op: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attrs: HashMap<String, Attr>,
}

impl Node {
    pub fn int(&self, name: &str, default: i64) -> i64 {
        match self.attrs.get(name) {
            Some(Attr::Int(v)) => *v,
            _ => default,
        }
    }

    pub fn ints(&self, name: &str) -> Option<&[i64]> {
        match self.attrs.get(name) {
            Some(Attr::Ints(v)) => Some(v),
            _ => None,
        }
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        match self.attrs.get(name) {
            Some(Attr::Str(v)) => Some(v),
            _ => None,
        }
    }
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub weights: HashMap<String, Tensor>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

/// Protobuf fields of a message: (field number, value)
enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
    Fixed64,
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn fields(b: &[u8]) -> Option<Vec<(u64, Value<'_>)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let value = match key & 7 {
            0 => Value::Varint(varint(b, &mut i)?),
            1 => {
                i += 8;
                Value::Fixed64
            }
            2 => {
                let n = varint(b, &mut i)? as usize;
                let v = b.get(i..i + n)?;
                i += n;
                Value::Bytes(v)
            }
            5 => {
                let v = u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?);
                i += 4;
                Value::Fixed32(v)
            }
            _ => return None,
        };
        out.push((key >> 3, value));
    }
    Some(out)
}

fn text(v: &Value) -> String {
    match v {
        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    }
}

/// Integers of a repeated field, packed or not
fn push_ints(v: &Value, out: &mut Vec<i64>) -> Option<()> {
    match v {
        Value::Varint(x) => out.push(*x as i64),
        Value::Bytes(b) => {
            let mut i = 0;
            while i < b.len() {
                out.push(varint(b, &mut i)? as i64);
            }
        }
        _ => {}
    }
    Some(())
}

fn tensor(b: &[u8]) -> Option<(String, Tensor)> {
    let (mut name, mut dims, mut data, mut kind) = (String::new(), Vec::new(), Vec::new(), 1);
    for (f, v) in fields(b)? {
        match f {
            1 => push_ints(&v, &mut dims)?,
            2 => {
                if let Value::Varint(k) = v {
                    kind = k;
                }
            }
            // float_data
            4 => match v {
                Value::Fixed32(x) => data.push(f32::from_bits(x)),
                Value::Bytes(b) => {
                    data.extend(b.as_chunks().0.iter().map(|c| f32::from_le_bytes(*c)))
                }
                _ => {}
            },
            // int64_data
            7 => {
                let mut ints = Vec::new();
                push_ints(&v, &mut ints)?;
                data.extend(ints.into_iter().map(|x| x as f32));
            }
            8 => name = text(&v),
            9 => {
                if let Value::Bytes(b) = v {
                    data = match kind {
                        1 => b
                            .as_chunks()
                            .0
                            .iter()
                            .map(|c| f32::from_le_bytes(*c))
                            .collect(),
                        7 => b
                            .as_chunks()
                            .0
                            .iter()
                            .map(|c| i64::from_le_bytes(*c) as f32)
                            .collect(),
                        _ => Vec::new(),
                    };
                }
            }
            _ => {}
        }
    }
    let dims = dims.into_iter().map(|d| d.max(0) as usize).collect();
    Some((name, Tensor { dims, data }))
}

fn attribute(b: &[u8]) -> Option<(String, Attr)> {
    let (mut name, mut attr, mut ints) = (String::new(), Attr::Other, Vec::new());
    for (f, v) in fields(b)? {
        match f {
            1 => name = text(&v),
            3 => {
                if let Value::Varint(x) = v {
                    attr = Attr::Int(x as i64);
                }
            }
            4 => attr = Attr::Str(text(&v)),
            5 => {
                if let Value::Bytes(b) = v {
                    attr = Attr::Tensor(tensor(b)?.1);
                }
            }
            8 => push_ints(&v, &mut ints)?,
            _ => {}
        }
    }
    if !ints.is_empty() {
        attr = Attr::Ints(ints);
    }
    Some((name, attr))
}

fn node(b: &[u8]) -> Option<Node> {
    let mut n = Node {
        op: String::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        attrs: HashMap::new(),
    };
    for (f, v) in fields(b)? {
        match f {
            1 => n.inputs.push(text(&v)),
            2 => n.outputs.push(text(&v)),
            4 => n.op = text(&v),
            5 => {
                if let Value::Bytes(b) = v {
                    let (name, attr) = attribute(b)?;
                    n.attrs.insert(name, attr);
                }
            }
            _ => {}
        }
    }
    Some(n)
}

/// Name of a graph input or output (ValueInfoProto)
fn value_name(b: &[u8]) -> Option<String> {
    fields(b)?
        .iter()
        .find(|(f, _)| *f == 1)
        .map(|(_, v)| text(v))
}

pub fn parse(model: &[u8]) -> Option<Graph> {
    let graph = fields(model)?.into_iter().find_map(|(f, v)| match (f, v) {
        (7, Value::Bytes(b)) => Some(b),
        _ => None,
    })?;
    let mut g = Graph {
        nodes: Vec::new(),
        weights: HashMap::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
    };
    for (f, v) in fields(graph)? {
        let Value::Bytes(b) = v else { continue };
        match f {
            1 => g.nodes.push(node(b)?),
            5 => {
                let (name, t) = tensor(b)?;
                g.weights.insert(name, t);
            }
            11 => g.inputs.push(value_name(b)?),
            12 => g.outputs.push(value_name(b)?),
            _ => {}
        }
    }
    // Weights are not inputs
    g.inputs.retain(|i| !g.weights.contains_key(i));
    Some(g)
}
