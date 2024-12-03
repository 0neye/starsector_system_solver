
#[derive(Clone, PartialEq)]
enum Token {
    Number(f32),
    Operator(Operator),
    LeftParen,
    RightParen,
}

#[derive(Clone, PartialEq)]
enum Operator {
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl Operator {
    pub fn precedence(&self) -> u8 {
        match self {
            Operator::Add | Operator::Subtract => 1,
            Operator::Multiply | Operator::Divide => 2,
        }
    }
}

fn tokenize(formula: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut number = String::new();

    for c in formula.chars() {
        match c {
            '0'..='9' | '.' => number.push(c),
            '+' | '-' | '*' | '/' | '(' | ')' => {
                if !number.is_empty() {
                    tokens.push(Token::Number(number.parse().unwrap()));
                    number.clear();
                }
                tokens.push(match c {
                    '+' => Token::Operator(Operator::Add),
                    '-' => Token::Operator(Operator::Subtract),
                    '*' => Token::Operator(Operator::Multiply),
                    '/' => Token::Operator(Operator::Divide),
                    '(' => Token::LeftParen,
                    ')' => Token::RightParen,
                    _ => unreachable!(),
                });
            }
            ' ' => {
                if !number.is_empty() {
                    tokens.push(Token::Number(number.parse().unwrap()));
                    number.clear();
                }
            }
            _ => panic!("Invalid character in formula"),
        }
    }

    if !number.is_empty() {
        tokens.push(Token::Number(number.parse().unwrap()));
    }

    tokens
}

fn apply_op(output: &mut Vec<f32>, op: Token) {
    if let Token::Operator(op) = op {
        let b = output.pop().unwrap();
        let a = output.pop().unwrap();
        let result = match op {
            Operator::Add => a + b,
            Operator::Subtract => a - b,
            Operator::Multiply => a * b,
            Operator::Divide => a / b,
        };
        output.push(result);
    }
}

pub fn calculate_formula(formula: &str, size: u32) -> f32 {
    let formula = formula.replace("size", &size.to_string());
    let tokens = tokenize(&formula);

    let mut output = Vec::new();
    let mut operators = Vec::new();

    for token in tokens {
        match token {
            Token::Number(num) => output.push(num),
            Token::LeftParen => operators.push(token),
            Token::RightParen => {
                while let Some(op) = operators.pop() {
                    if op == Token::LeftParen {
                        break;
                    }
                    apply_op(&mut output, op);
                }
            },
            Token::Operator(op) => {
                while let Some(Token::Operator(top)) = operators.last() {
                    if op.precedence() <= top.precedence() {
                        apply_op(&mut output, operators.pop().unwrap());
                    } else {
                        break;
                    }
                }
                operators.push(Token::Operator(op));
            },
        }
    }

    while let Some(op) = operators.pop() {
        apply_op(&mut output, op);
    }

    output.pop().unwrap()

}